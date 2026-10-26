use crate::settings::{Fragment, VideoPath};
use anyhow::{Context, Result};
use libmpv2::{events::*, *};
use log::{debug, info, warn};
use std::collections::LinkedList;
use strum::{Display, EnumString};

#[derive(Clone, Copy, Display, EnumString)]
pub(crate) enum Role {
    Main,
    Outro,
    Intro,
}

#[derive(Clone)]
pub struct Clip {
    path: VideoPath,
    role: Role,
    is_loop: bool,
}

impl Fragment {
    fn to_clips(&self) -> LinkedList<Clip> {
        let mut queue = LinkedList::new();
        let Fragment {
            intro,
            main,
            outro,
            looping,
            ..
        } = self;
        if let Some(outro) = outro {
            queue.push_front(Clip {
                path: outro.clone(),
                role: Role::Outro,
                is_loop: false,
            });
        }
        queue.push_front(Clip {
            path: main.clone(),
            role: Role::Main,
            is_loop: looping.clone(),
        });
        if let Some(intro) = intro {
            queue.push_front(Clip {
                path: intro.clone(),
                role: Role::Intro,
                is_loop: false,
            });
        }
        queue
    }
}

pub struct Player {
    queue: LinkedList<Clip>,
    idle: Fragment,
}

impl Player {
    pub fn new(idle: Fragment) -> Self {
        Player {
            queue: LinkedList::new(),
            idle: idle,
        }
    }
    pub fn play(&mut self, fragment: &Fragment) -> Clip {
        let current = self.queue.pop_front();
        let next = self.queue.pop_front();
        self.queue = LinkedList::new();
        // If playing a `main` clip currently that has a queued `outro`, then play it first
        // before the rest of the new clips
        if let (
            Some(Clip {
                role: Role::Main, ..
            }),
            Some(
                next @ Clip {
                    role: Role::Outro, ..
                },
            ),
        ) = (current, next)
        {
            debug!("Found outro, queueing it to play first...");
            self.queue.push_front(next);
        }
        self.queue.append(&mut fragment.to_clips());
        self.queue
            .front()
            .expect("At least one clip should always be present with the current implementation")
            .clone()
    }

    pub fn stop(&mut self) -> Clip {
        self.queue = LinkedList::new();
        self.queue.append(&mut self.idle.to_clips());
        self.queue
            .front()
            .expect("At least one clip should always be present with the current implementation")
            .clone()
    }

    pub fn process_eof(&mut self) -> Clip {
        debug!("Processing eof...");
        let _current = self.queue.pop_front();
        match self.queue.front() {
            // Play next if there is any
            Some(clip) => clip.clone(),
            // Or play idle if there is none
            None => self.stop(),
        }
    }
}

pub trait MpvControls {
    fn render(&self, clip: Clip) -> Result<()>;
    fn set_pause(&self, pause: bool) -> Result<()>;
    fn toggle_pause(&self) -> Result<()>;
    fn change_vol(&self, vol_delta: i8) -> Result<()>;
}

impl MpvControls for Mpv {
    fn render(&self, clip: Clip) -> Result<()> {
        let Clip {
            path,
            role: _role,
            is_loop,
        } = clip;
        let path = path.to_str().context("Decoding path failed")?;
        info!("Rendering clip {}", path);
        self.command(
            "loadfile",
            &[
                path,
                "replace",
                "0",
                &format!("loop-file={}", if is_loop { "inf" } else { "no" }),
            ],
        )
        .context(format!("Rendering {path} failed"))?;
        self.set_pause(false)
    }

    fn set_pause(&self, pause: bool) -> Result<()> {
        self.set_property("pause", pause)
            .context("Setting pause state failed")
    }

    fn toggle_pause(&self) -> Result<()> {
        self.command("cycle", &["pause"])
            .context("Toggling pause failed")
    }

    fn change_vol(&self, vol_delta: i8) -> Result<()> {
        let vol: f64 = self
            .get_property("volume")
            .context("Couldn't read volume")?;
        self.set_property("volume", (vol + f64::from(vol_delta)).clamp(0.0, 100.0))
            .context("Couldn't set volume")
    }
}
