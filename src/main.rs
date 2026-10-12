use anyhow::Result;
use axum::{
    Router,
    extract::{Path, State},
    response::Html,
    routing::{get, post},
};
use libmpv2::{events::*, *};
use log::{LevelFilter, error, info, warn};
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

#[tokio::main]
async fn main() -> Result<()> {
    if connected_to_journal() {
        JournalLog::new()
            .unwrap()
            .with_extra_fields(vec![("VERSION", env!("CARGO_PKG_VERSION"))])
            .install()
            .unwrap();
    } else {
        colog::init();
    }

    log::set_max_level(LevelFilter::Info);

    let mpv = Mpv::new().expect("to start the player");
    mpv.command(
        "loadfile",
        &["/home/gera/downloads/02beg_low.mkv", "append-play", "0", "loop-file=0"],
    )
    .ok();
    mpv.command(
        "loadfile",
        &["/home/gera/downloads/02beg_low.mkv", "append-play", "0", "loop-file=0"],
    )
    .ok();

    let (tx, mut rx) = mpsc::channel::<Command>(8);

    // tokio::spawn(async move {
    //     mpv.set_property("audio-device", "pipewire/combined").ok();
    //     mpv.set_property("force-window", true).ok();
    //     mpv.command(
    //         "loadfile",
    //         &["/home/gera/downloads/02beg_low.mkv", "loop-file=inf"],
    //     )
    //     .ok();
    // });

    let app = Router::new()
        .route("/", get(index))
        .route("/cmd/{name}", post(cmd))
        .with_state(tx);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:8080").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
