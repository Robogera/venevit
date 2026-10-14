mod settings;
use anyhow::Context;
use anyhow::Result;
use axum::{
    Router,
    extract::{Path, State},
    response::Html,
    routing::{get, post},
};
use libmpv2::{events::*, *};
use log::{LevelFilter, error, info, warn};
use settings::{Fragment, Settings};
use strum::{Display, EnumString};
use systemd_journal_logger::{JournalLog, connected_to_journal};
use tokio::sync::mpsc;

enum Command {
    Start,
    Stop,
    Pause,
    VolUp,
    VolDown,
}

const PAGE: &str = r#"<!doctype html><html><body style="background:#fff">
<form method="post" action="/cmd/start"><button>Start</button></form>
<form method="post" action="/cmd/stop"><button>Stop</button></form>
<form method="post" action="/cmd/pause"><button>Pause</button></form>
<form method="post" action="/cmd/volup"><button>Vol Up</button></form>
<form method="post" action="/cmd/voldown"><button>Vol Down</button></form>
</body></html>"#;

async fn index() -> Html<&'static str> {
    Html(PAGE)
}

async fn cmd(
    State(tx): State<mpsc::Sender<Command>>,
    Path(name): Path<String>,
) -> Html<&'static str> {
    let c = match name.as_str() {
        "start" => Command::Start,
        "stop" => Command::Stop,
        "pause" => Command::Pause,
        "volup" => Command::VolUp,
        "voldown" => Command::VolDown,
        _ => return Html(PAGE),
    };
    let _ = tx.send(c).await;
    Html(PAGE)
}

enum PlayMethod {
    Immediately,
    Queue,
}

#[derive(Clone, Copy, Display, EnumString)]
enum VideoType {
    Main,
    Transition,
}

impl VideoType {
    fn new(tag: &str) -> Result<Self> {
        let video_type = tag
            .parse()
            .context(format!("{tag} is not a valid VideoType member!"))?;
        Ok(video_type)
    }

    fn tag(self) -> String {
        format!("force-media-title={self}")
    }
}

struct Clip {
    path: String,
    is_loop: bool,
    video_type: VideoType,
}

trait PlayerControl {
    fn send_play_command(
        &self,
        path: &str,
        is_loop: bool,
        video_type: VideoType,
        method: PlayMethod,
    ) -> Result<()>;
    fn play(&self, fragment: Fragment) -> Result<()>;
    fn clear_playlist(&self) -> Result<()>;
    fn toggle_pause(&self) -> Result<()>;
    fn ensure_not_paused(&self) -> Result<()>;
    fn next(&self) -> Result<()>;
}

impl PlayerControl for Mpv {
    fn play(&self, fragment: Fragment) -> Result<()> {
        let mut queue: Vec<(&str, bool, VideoType)> = vec![];
        if let Some(intro) = fragment.intro.as_ref() {
            queue.push((
                intro.to_str().ok_or(Error::InvalidUtf8)?,
                false,
                VideoType::Transition,
            ));
        }
        queue.push((
            fragment.main.to_str().ok_or(Error::InvalidUtf8)?,
            fragment.looping,
            VideoType::Main,
        ));
        if let Some(outro) = fragment.outro.as_ref() {
            queue.push((
                outro.to_str().ok_or(Error::InvalidUtf8)?,
                false,
                VideoType::Transition,
            ));
        }
        match VideoType::new(&self.get_property::<String>("media-title")?)? {
            VideoType::Main => {
                for (path, looping, video_type) in queue {
                    self.send_play_command(path, looping, video_type, PlayMethod::Queue)?;
                    self.next()?;
                }
            }
            VideoType::Transition => {
                let ((path, looping, video_type), rest) = queue
                    .split_first()
                    .ok_or(anyhow::Error::msg("Empty clip queue"))?;
                self.send_play_command(path, *looping, *video_type, PlayMethod::Immediately)?;
                for (path, looping, video_type) in rest {
                    self.send_play_command(path, *looping, *video_type, PlayMethod::Queue)?;
                }
            }
        };
        Ok(())
    }
    fn send_play_command(
        &self,
        path: &str,
        is_loop: bool,
        video_type: VideoType,
        method: PlayMethod,
    ) -> Result<()> {
        self.command(
            "loadfile",
            &[
                path,
                match method {
                    PlayMethod::Immediately => "append-play",
                    PlayMethod::Queue => "append",
                },
                "0",
                &format!("loop-file={}", is_loop as u8),
                &video_type.tag(),
            ],
        )
        .context("Play command failed")?;
        Ok(())
    }

    fn clear_playlist(&self) -> Result<()> {
        self.command("playlist-clear", &[])
            .context("Clear playlist command failed")?;
        Ok(())
    }

    fn toggle_pause(&self) -> Result<()> {
        self.command("cycle", &["pause"])
            .context("Pause toggle failed")?;
        Ok(())
    }

    fn ensure_not_paused(&self) -> Result<()> {
        self.command("set", &["pause", "yes"])
            .context("Unpause command failed")?;
        Ok(())
    }

    fn next(&self) -> Result<()> {
        self.command("playlist-next", &[])
            .context("Playlist next command failed")?;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let s = Settings::new()?;

    if connected_to_journal() {
        JournalLog::new()?
            .with_extra_fields(vec![("VERSION", env!("CARGO_PKG_VERSION"))])
            .install()?;
    } else {
        colog::init();
    }

    log::set_max_level(LevelFilter::Info);

    let mpv = Mpv::with_initializer(|m| {
        m.set_property("audio-device", "pipewire/combined")?;
        m.set_property("idle", "yes")?;
        m.set_property("keep-open", "yes")?;
        Ok(())
    })?;

    let (tx, mut rx) = mpsc::channel::<Command>(8);

    tokio::spawn(async move {
        mpv.command(
            "loadfile",
            &[
                "video/background_loop.mkv",
                "append-play",
                "0",
                "loop-file=1",
            ],
        )
        .ok();
    });

    let app = Router::new()
        .route("/", get(index))
        .route("/cmd/{name}", post(cmd))
        .with_state(tx);

    let listener =
        tokio::net::TcpListener::bind(format!("0.0.0.0:{:?}", s.http_port.unwrap_or(8080))).await?;
    axum::serve(listener, app).await?;
    Ok(())
}
