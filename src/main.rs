mod player;
mod settings;
use anyhow::{Context, Result};
use async_anyhow_logger::catch_with_context;
use axum::{
    Router,
    extract::{Path, State},
    response::Html,
    routing::{get, post},
};
use html_escape::encode_text;
use libmpv2::*;
use log::{LevelFilter, debug, error, info, warn};
use pjlink::PjlinkDevice;
use player::{MpvControls, Player};
use settings::{Fragment, Projector, Settings};
use std::time::Duration;
use std::{iter::Peekable, sync::Arc};
use systemd_journal_logger::{JournalLog, connected_to_journal};
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval};

#[derive(Debug, Clone, Copy)]
enum Command {
    Play(usize),
    Stop,
    Pause,
    VolUp,
    VolDown,
    ProjectorsOn,
    ProjectorsOff,
}

#[derive(Clone)]
struct AppState {
    tx: mpsc::Sender<Command>,
    page: Arc<str>,
}

impl AppState {
    fn new(playlist: &[Fragment], tx: mpsc::Sender<Command>) -> Self {
        let buttons: String = playlist
            .iter()
            .enumerate()
            .map(|(i, f)| {
                format!(
                    r#"<form method="post" action="/cmd/{i}"><button>{}</button></form>"#,
                    encode_text(&f.name)
                )
            })
            .collect();

        AppState {
            tx,
            page: Arc::from(format!(
                r#"<!doctype html><html><head><meta charset="utf-8"><title>venevit</title></head>
<body style="background:#fff">
{buttons}
<form method="post" action="/cmd/stop"><button>Стоп</button></form>
<form method="post" action="/cmd/pause"><button>Пауза/Продолжить</button></form>
<form method="post" action="/cmd/volup"><button>Громкость +</button></form>
<form method="post" action="/cmd/voldown"><button>Громкость -</button></form>
<form method="post" action="/cmd/projectorson"><button>Стенд ВКЛ</button></form>
<form method="post" action="/cmd/projectorsoff"><button>Стенд ВЫКЛ</button></form>
</body></html>"#
            )),
        }
    }
    fn html(&self) -> Html<String> {
        Html(self.page.to_string())
    }

    async fn index(State(app): State<Self>) -> Html<String> {
        app.html()
    }

    async fn cmd(State(app): State<Self>, Path(key): Path<String>) -> Html<String> {
        let command = match key.as_str() {
            "stop" => Some(Command::Stop),
            "pause" => Some(Command::Pause),
            "volup" => Some(Command::VolUp),
            "voldown" => Some(Command::VolDown),
            "projectorsoff" => Some(Command::ProjectorsOff),
            "projectorson" => Some(Command::ProjectorsOn),
            _ => key.parse().ok().map(Command::Play),
        };
        if let Some(command) = command {
            let _ = app.tx.send(command).await;
        }
        app.html()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let Settings {
        playlist,
        idle,
        http_port,
        projectors,
        ..
    } = Settings::new()?;

    if connected_to_journal() {
        JournalLog::new()?
            .with_extra_fields(vec![("VERSION", env!("CARGO_PKG_VERSION"))])
            .install()?;
    } else {
        env_logger::init();
    }

    log::set_max_level(LevelFilter::Info);

    let mpv = Mpv::with_initializer(|m| {
        m.set_property("audio-device", "pipewire/combined")?;
        m.set_property("idle", "yes")?;
        m.set_property("keep-open", "yes")?;
        m.set_property("force-window", "immediate")?;
        m.set_property("vo", "x11")?;
        Ok(())
    }).context("libMPV init failure")?;

    let (tx, mut rx) = mpsc::channel::<Command>(8);

    let state = AppState::new(&playlist, tx);

    let mut player = Player::new(idle);

    let mut ticker = interval(Duration::from_millis(100));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut was_eof = false;
    let player_handle = tokio::spawn(catch_with_context(|| "player worker", async move {
        mpv.render(player.stop())?;
        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    apply(&mpv, &mut player, &playlist, &projectors, cmd.context("Invalid command")?)?;
                }
                _ = ticker.tick() => {
                    let is_eof = matches!(mpv.get_property::<bool>("eof-reached"), Ok(true));
                    if (false, true) == (was_eof, is_eof) {
                         mpv.render(player.process_eof())?;
                    }
                    was_eof = is_eof;
                }
            }
        }
    }));

    let app = Router::new()
        .route("/", get(AppState::index))
        .route("/cmd/{key}", post(AppState::cmd))
        .with_state(state);

    let listener =
        tokio::net::TcpListener::bind(format!("0.0.0.0:{:?}", http_port.unwrap_or(8080))).await?;

    tokio::select! {
        r = axum::serve(listener, app) => { r?; }
        _ = tokio::signal::ctrl_c() => { info!("shutting down"); }
        joined = player_handle => {
            match joined {
                Ok(()) => error!("Player exited unexpectedly"),
                Err(e) => error!("Error: {:#}", e),
            }
        }
    }
    Ok(())
}

fn apply(
    mpv: &Mpv,
    player: &mut Player,
    playlist: &[Fragment],
    projectors: &[Projector],
    cmd: Command,
) -> Result<()> {
    match cmd {
        Command::Play(i) => {
            info!("Playing fragment {i}");
            mpv.render(player.play(playlist.get(i).context("bad playlist index")?))
                .context("Rendering the chosen fragment failed")
        }
        Command::Pause => {
            info!("Toggling pause");
            mpv.toggle_pause()
        }
        Command::VolUp => {
            info!("Volume up");
            mpv.change_vol(7)
        }
        Command::VolDown => {
            info!("Volume up");
            mpv.change_vol(-7)
        }
        Command::Stop => {
            info!("Stop");
            mpv.render(player.stop()).context("Stop failed")
        }
        Command::ProjectorsOn => {
            info!("Projectors: ON");
            set_projectors_power(projectors, true)
        }
        Command::ProjectorsOff => {
            info!("Projectors: OFF");
            set_projectors_power(projectors, false)
        }
    }
}

fn set_projectors_power(projectors: &[Projector], on: bool) -> Result<()> {
    let action = if on {
        PjlinkDevice::power_on
    } else {
        PjlinkDevice::power_off
    };
    let handles: Vec<_> = projectors
        .iter()
        .map(|projector| {
            let ip = projector.ip.to_string();
            tokio::task::spawn_blocking(move || -> Result<()> {
                PjlinkDevice::new(&ip)
                    .context("Couldn't create a Pjlink device")
                    .and_then(|mut dev| {
                        action(&mut dev)
                            .map(drop)
                            .context("Couldn't send command to Pjlink device")
                    })
            })
        })
        .collect();

    tokio::task::block_in_place(|| futures::executor::block_on(futures::future::join_all(handles)))
        .into_iter()
        .map(|r| r.context("asdasd").and_then(|inner| inner))
        .fold(Ok(()), |acc, r| {
            if let Err(e) = &r {
                warn!("{e:#}");
            }
            acc.and(r)
        })
}
