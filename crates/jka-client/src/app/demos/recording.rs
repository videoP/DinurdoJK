//! Demo recording.
use crate::app::{demo, App, File, PathBuf};
use std::io::Write;

#[cfg(windows)]
pub(in crate::app) fn demo_recording_timestamp() -> String {
    #[repr(C)]
    struct WinSystemTime {
        year: u16,
        month: u16,
        day_of_week: u16,
        day: u16,
        hour: u16,
        minute: u16,
        second: u16,
        milliseconds: u16,
    }
    #[link(name = "Kernel32")]
    extern "system" {
        fn GetLocalTime(system_time: *mut WinSystemTime);
    }
    let mut local = std::mem::MaybeUninit::<WinSystemTime>::uninit();
    unsafe {
        GetLocalTime(local.as_mut_ptr());
        let local = local.assume_init();
        format!(
            "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
            local.year, local.month, local.day, local.hour, local.minute, local.second
        )
    }
}

#[cfg(not(windows))]
pub(in crate::app) fn demo_recording_timestamp() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
        .to_string()
}

pub(in crate::app) struct DemoRecording {
    pub(in crate::app) file: File,
    pub(in crate::app) path: PathBuf,
    pub(in crate::app) waiting_for_full_snapshot: bool,
}

impl App {
    pub(in crate::app) fn start_demo_recording(&mut self, requested_name: Option<&str>) {
        use jka_protocol::session::ConnectionState;

        if self.demo_recording.is_some() {
            self.push_console_line("^3Already recording.".to_owned());
            return;
        }
        let active = self
            .net
            .as_ref()
            .is_some_and(|net| net.session().state() == ConnectionState::Active);
        if !active {
            self.push_console_line("^3You must be in a level to record.".to_owned());
            return;
        }

        let mut stem = requested_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| name.trim_matches('"').replace('\\', "/"))
            .unwrap_or_else(|| format!("demo{}", demo_recording_timestamp()));
        if let Some(stripped) = stem.strip_prefix("demos/") {
            stem = stripped.to_owned();
        }
        if let Some(stripped) = stem.strip_suffix(".dm_26") {
            stem = stripped.to_owned();
        }
        let relative = PathBuf::from(&stem);
        if stem.is_empty()
            || relative.components().any(|component| {
                matches!(
                    component,
                    std::path::Component::ParentDir
                        | std::path::Component::RootDir
                        | std::path::Component::Prefix(_)
                )
            })
        {
            self.push_console_line("^1record name must stay inside demos/".to_owned());
            return;
        }

        let qpath = PathBuf::from("demos").join(format!("{stem}.dm_26"));
        let destination = self.game_write_path(&qpath);
        if let Some(parent) = destination.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                self.push_console_path_line(
                    format!("^1Couldn't create {}: {error}", parent.display()),
                    parent.to_path_buf(),
                );
                return;
            }
        }

        let initial = self.net.as_ref().map(|net| {
            let session = net.session();
            demo::synthesize_gamestate_payload(session.decoder(), session.reliable_sequence())
                .map(|payload| (session.server_message_sequence().wrapping_sub(1), payload))
        });
        let (sequence, payload) = match initial {
            Some(Ok(initial)) => initial,
            Some(Err(error)) => {
                self.push_console_line(format!("^1Could not build demo gamestate: {error}"));
                return;
            }
            None => {
                self.push_console_line("^3You must be in a level to record.".to_owned());
                return;
            }
        };

        let mut file = match File::create(&destination) {
            Ok(file) => file,
            Err(error) => {
                self.push_console_path_line(
                    format!("^1ERROR: couldn't open {}: {error}", destination.display()),
                    destination.clone(),
                );
                return;
            }
        };
        if let Err(error) = demo::write_record(&mut file, sequence, &payload) {
            self.push_console_path_line(
                format!("^1ERROR: couldn't write {}: {error}", destination.display()),
                destination.clone(),
            );
            return;
        }

        self.demo_recording = Some(DemoRecording {
            file,
            path: destination.clone(),
            waiting_for_full_snapshot: true,
        });
        if let Some(net) = self.net.as_mut() {
            net.session_mut().set_demo_capture(true);
        }
        self.push_console_path_line(
            format!("^2recording to {}.", destination.display()),
            destination.clone(),
        );
    }

    pub(in crate::app) fn record_demo_message(
        &mut self,
        sequence: i32,
        payload: Vec<u8>,
        full_snapshot: bool,
    ) {
        let Some(recording) = self.demo_recording.as_mut() else {
            return;
        };
        if recording.waiting_for_full_snapshot {
            if !full_snapshot {
                return;
            }
            recording.waiting_for_full_snapshot = false;
        }
        if let Err(error) = demo::write_record(&mut recording.file, sequence, &payload) {
            let path = recording.path.clone();
            self.demo_recording = None;
            if let Some(net) = self.net.as_mut() {
                net.session_mut().set_demo_capture(false);
            }
            self.push_console_path_line(
                format!("^1Demo recording failed for {}: {error}", path.display()),
                path,
            );
        }
    }

    pub(in crate::app) fn stop_demo_recording(&mut self) {
        let Some(mut recording) = self.demo_recording.take() else {
            self.push_console_line("^3Not recording a demo.".to_owned());
            return;
        };
        if let Some(net) = self.net.as_mut() {
            net.session_mut().set_demo_capture(false);
        }
        let result = demo::write_end(&mut recording.file).and_then(|()| recording.file.flush());
        match result {
            Ok(()) => self.push_console_path_line(
                format!("^2Stopped demo: {}", recording.path.display()),
                recording.path.clone(),
            ),
            Err(error) => self.push_console_path_line(
                format!(
                    "^1Error finishing demo {}: {error}",
                    recording.path.display()
                ),
                recording.path.clone(),
            ),
        }
    }
}
