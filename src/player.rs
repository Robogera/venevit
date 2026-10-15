use crate::settings::Fragment;
use anyhow::{Context, Result};
use libmpv2::{events::*, *};
use strum::{Display, EnumString};

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
    fn new(tag: &str) -> Option<Self> {
        let video_type: Result<Self, _> = tag.parse();
        video_type.ok()
    }

    fn tag(self) -> String {
        format!("force-media-title={self}")
    }
}

pub(crate) trait PlayerControl {
    fn send_play_command(
        &self,
        path: &str,
        is_loop: bool,
        video_type: VideoType,
        method: PlayMethod,
    ) -> Result<()>;
    fn play(&self, fragment: &Fragment) -> Result<()>;
    fn clear_playlist(&self) -> Result<()>;
    fn toggle_pause(&self) -> Result<()>;
    fn ensure_not_paused(&self) -> Result<()>;
    fn next(&self) -> Result<()>;
}

impl PlayerControl for Mpv {
    fn play(&self, fragment: &Fragment) -> Result<()> {
        let mut queue: Vec<(&str, bool, VideoType)> = vec![];

        // Add the intro clip if any
        if let Some(intro) = fragment.intro.as_ref() {
            queue.push((
                intro.to_str().ok_or(Error::InvalidUtf8)?,
                false,
                VideoType::Transition,
            ));
        }

        // Add the main clip
        queue.push((
            fragment.main.to_str().ok_or(Error::InvalidUtf8)?,
            fragment.looping,
            VideoType::Main,
        ));

        // Add the outro clip if any
        if let Some(outro) = fragment.outro.as_ref() {
            queue.push((
                outro.to_str().ok_or(Error::InvalidUtf8)?,
                false,
                VideoType::Transition,
            ));
        }

        match VideoType::new(
            &self
                .get_property::<String>("media-title")
                .unwrap_or("nothing".to_owned()),
        ) {
            // If the main video is playing, queue the new clips and play next
            // To ensure the outro of the currently playing clip plays correctly (if any)
            Some(VideoType::Main) | None => {
                for (path, looping, video_type) in queue {
                    self.send_play_command(path, looping, video_type, PlayMethod::Queue)?;
                    self.next()?;
                }
            }

            // If the transition (intro/outro) clip is playing, just start playing the new
            // clips immediately to both give the immediate feedback on button press and also
            // make it less jarring visually (all transitions are similar-ish enough being
            // grey ghost fog SFX)
            Some(VideoType::Transition) => {
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
                &format!("loop-file={},{}", is_loop as u8, video_type.tag()),
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
