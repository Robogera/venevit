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
use log::{LevelFilter, error, info, warn};
use pjlink::PjlinkDevice;
use player::{MpvControls, Player};
use settings::{Fragment, Projector, Settings};
use std::time::Duration;
use std::sync::Arc;
use systemd_journal_logger::{JournalLog, connected_to_journal};
use tokio::sync::mpsc;
use tokio::time::{MissedTickBehavior, interval};
use tower_http::services::ServeDir;

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
                    r#"<form method="post" action="/cmd/{i}"><button class="vid">{}</button></form>"#,
                    encode_text(&f.name)
                )
            })
            .collect();

        AppState {
            tx,
            page: Arc::from(format!(
                r#"<!doctype html><html lang="ru"><head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Музей-Усадьба Дмитрия Веневитинова</title>
<style>
*{{box-sizing:border-box}}
html,body{{min-height:100%}}
body{{margin:0 auto;padding:1rem;max-width:32rem;
 background-color:#f5f4ef;
 background-image:linear-gradient(rgba(255,255,255,.60),rgba(255,255,255,.88)),
                  url("/assets/bg.jpg");
 background-position:center center;
 background-size:cover;
 background-repeat:no-repeat;
 color:#141414;
 font-family:Georgia,"Times New Roman","Noto Serif",serif,'Apple Color Emoji','Segoe UI Emoji','Noto Color Emoji';
 text-align:center;-webkit-text-size-adjust:100%}}
h1{{font-size:1.35rem;font-weight:600;margin:0 0 clamp(0rem,4svh,8rem);
 color:rgba(20,20,20,.82)}}
h2{{font-size:1.1rem;font-weight:600;letter-spacing:.02em;margin:1.5rem 0 .75rem;
 color:rgba(20,20,20,.82)}}
form{{margin:0}}
.row{{display:flex;gap:.75rem;justify-content:center;margin:0 0 .75rem}}
.row form{{flex:1 1 0;display:flex}}
.icon{{flex:1 1 0;background:none;border:0;padding:.5rem;font-size:2.75rem;line-height:1;
 color:inherit;cursor:pointer;touch-action:manipulation;-webkit-tap-highlight-color:transparent}}
.vid{{display:block;width:100%;margin:0 0 .75rem;padding:1rem;font-size:1.05rem;
 font-family:inherit;color:inherit;
 background:rgba(255,255,255,.10);
 border:1px solid rgba(20,20,20,.28);
 border-radius:.75rem;cursor:pointer;touch-action:manipulation;
 word-break:break-word;overflow-wrap:anywhere}}
.icon:active,.vid:active{{opacity:.55}}
</style>
</head><body>
<body>
<h1>Музей-усадьба Д.В. Веневитинова</h1>
{buttons}
<h2>Управление</h2>
<div class="row">
<form method="post" action="/cmd/pause"><button class="icon">&#x23EF;&#xFE0F;</button></form>
<form method="post" action="/cmd/stop"><button class="icon">&#x23F9;&#xFE0F;</button></form>
<form method="post" action="/cmd/volup"><button class="icon">&#x1F50A;</button></form>
<form method="post" action="/cmd/voldown"><button class="icon">&#x1F509;</button></form>
</div>
<h2>Проекторы</h2>
<div class="row">
<form method="post" action="/cmd/projectorson"><button class="icon">&#x1F7E2;&#xFE0F;</button></form>
<form method="post" action="/cmd/projectorsoff"><button class="icon">&#x1F534;&#xFE0F;</button></form>
</div>
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
    })
    .context("libMPV init failure")?;

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
        .nest_service("/assets", ServeDir::new("assets"))
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
            info!("webserver command: playing fragment {i}");
            mpv.render(player.play(playlist.get(i).context("bad playlist index")?))
                .context("render failed")
        }
        Command::Pause => {
            info!("webserver command: toggle pause");
            mpv.toggle_pause()
        }
        Command::VolUp => {
            info!("webserver command: volume up");
            mpv.change_vol(7)
        }
        Command::VolDown => {
            info!("webserver command: volume down");
            mpv.change_vol(-7)
        }
        Command::Stop => {
            info!("webserver command: stop");
            mpv.render(player.stop()).context("Stop failed")
        }
        Command::ProjectorsOn => {
            info!("webserver command: projectors on");
            projector_bulk_command(projectors, |dev| {
                dev.power_on().context("power on command failed").map(drop)
            })
            .or_else(|e| {
                warn!("{e:#}");
                Ok(())
            })
        }
        Command::ProjectorsOff => {
            info!("webserver command: projectors off");
            projector_bulk_command(projectors, |dev| {
                dev.power_off()
                    .context("power off command failed")
                    .map(drop)
            })
            .or_else(|e| {
                warn!("{e:#}");
                Ok(())
            })
        }
    }
}

fn projector_bulk_command(
    projectors: &[Projector],
    action: fn(&PjlinkDevice) -> Result<()>,
) -> Result<()> {
    let handles: Vec<_> = projectors
        .iter()
        .map(|projector| {
            let ip = projector.ip.to_string();
            tokio::task::spawn_blocking(move || -> Result<()> {
                PjlinkDevice::new(&ip)
                    .context(format!("{ip:#} init error"))
                    .and_then(|mut dev| {
                        action(&mut dev)
                            .map(drop)
                            .context(format!("{ip:#} command failed"))
                    })
            })
        })
        .collect();

    tokio::task::block_in_place(|| futures::executor::block_on(futures::future::join_all(handles)))
        .into_iter()
        .map(|r| r.context("handle join error").and_then(|inner| inner))
        .fold(Ok(()), |acc, r| {
            if let Err(e) = &r {
                error!("{e:#}");
            }
            acc.and(r)
        })
        .map_err(|_| anyhow::Error::msg("bulk command failed for some projectors"))
}
