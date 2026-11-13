//! Demo recording.
use crate::app::{demo, App, File, PathBuf};
use std::io::Write;
use std::path::Path;

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
    // All map segments belong to the path selected by the original /record.
    base_path: PathBuf,
    next_part: u32,
    // The next DemoMessage contains the new map's real wire gamestate.
    rollover_pending: bool,
}

/// Each gamestate is a self-contained demo. Do not put successive maps into
/// the same stream: their serverTime and baselines are independent.
fn demo_segment_path(base: &Path, part: u32) -> PathBuf {
    let stem = base.file_stem().expect("recording path has a stem").to_string_lossy();
    let extension = base.extension().expect("recording path has a protocol extension").to_string_lossy();
    base.with_file_name(format!("{stem}_part{part:03}.{extension}"))
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

        let jk2 = self.net.as_ref().and_then(|net| net.session().jk2_version());
        let extension = jk2.map_or("dm_26".to_owned(), |version| format!("dm_{}", version.protocol()));
        // JK2MV CL_Record_f chooses the first free demo0000..demo9999 when
        // no name is supplied. Preserve the existing JKA timestamp default.
        let auto_name = if jk2.is_some() && requested_name.is_none_or(|name| name.trim().is_empty()) {
            (0..=9999).find_map(|number| {
                let stem = format!("demo{number:04}");
                (!self.game_write_path(format!("demos/{stem}.{extension}")).exists()).then_some(stem)
            })
        } else {
            None
        };
        if jk2.is_some() && requested_name.is_none_or(|name| name.trim().is_empty()) && auto_name.is_none() {
            self.push_console_line("^1No free JK2 demo0000..demo9999 filename.");
            return;
        }
        let mut stem = requested_name
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(|name| name.trim_matches('"').replace('\\', "/"))
            .unwrap_or_else(|| auto_name.unwrap_or_else(|| format!("demo{}", demo_recording_timestamp())));
        if let Some(stripped) = stem.strip_prefix("demos/") {
            stem = stripped.to_owned();
        }
        if stem.to_ascii_lowercase().ends_with(&format!(".{extension}")) {
            stem.truncate(stem.len() - extension.len() - 1);
        } else if jk2.is_some() && (stem.to_ascii_lowercase().ends_with(".dm_15")
            || stem.to_ascii_lowercase().ends_with(".dm_16"))
        {
            self.push_console_line(format!("^1record name has the wrong JK2 protocol extension (expected .{extension})."));
            return;
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

        let qpath = PathBuf::from("demos").join(format!("{stem}.{extension}"));
        let destination = self.game_write_path(&qpath);
        self.begin_demo_recording(destination.clone(), destination, 2, false, None);
    }

    /// A manual /record synthesizes its initial gamestate; a map rollover
    /// starts with the server's actual gamestate message. The latter avoids
    /// stale decoder state and packet sequence reordering if the network pump
    /// has already parsed further packets before delivering their events.
    fn begin_demo_recording(
        &mut self,
        destination: PathBuf,
        base_path: PathBuf,
        next_part: u32,
        new_segment: bool,
        incoming_gamestate: Option<(i32, Vec<u8>)>,
    ) -> bool {
        if let Some(parent) = destination.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                self.push_console_path_line(
                    format!("^1Couldn't create {}: {error}", parent.display()),
                    parent.to_path_buf(),
                );
                return false;
            }
        }

        let (sequence, payload) = if let Some(gamestate) = incoming_gamestate {
            gamestate
        } else {
            let initial = self.net.as_ref().map(|net| {
                let session = net.session();
                // JK2MV CL_Record_f writes the native JK2 wire gamestate, not
                // the presentation decoder's translated JKA configstrings.
                let payload = if let Some(wire) = session.jk2_decoder() {
                    jka_protocol::jk2::demo::synthesize_gamestate_payload(wire, session.reliable_sequence())
                } else {
                    demo::synthesize_gamestate_payload(session.decoder(), session.reliable_sequence())
                };
                payload.map(|payload| (session.server_message_sequence().wrapping_sub(1), payload))
            });
            match initial {
                Some(Ok(initial)) => initial,
                Some(Err(error)) => {
                    self.push_console_line(format!("^1Could not build demo gamestate: {error}"));
                    return false;
                }
                None => {
                    self.push_console_line("^3You must be in a level to record.".to_owned());
                    return false;
                }
            }
        };

        let file_result = if new_segment {
            // A map change must never overwrite an older segment, even if a
            // same-named recording was made earlier or another process races us.
            std::fs::OpenOptions::new().write(true).create_new(true).open(&destination)
        } else {
            File::create(&destination)
        };
        let mut file = match file_result {
            Ok(file) => file,
            Err(error) => {
                self.push_console_path_line(
                    format!("^1ERROR: couldn't open {}: {error}", destination.display()),
                    destination.clone(),
                );
                return false;
            }
        };
        if let Err(error) = demo::write_record(&mut file, sequence, &payload) {
            self.push_console_path_line(
                format!("^1ERROR: couldn't write {}: {error}", destination.display()),
                destination.clone(),
            );
            return false;
        }

        self.demo_recording = Some(DemoRecording {
            file,
            path: destination.clone(),
            // A raw map gamestate needs no synthetic prelude or snapshot
            // gating: all following messages belong to this new map.
            waiting_for_full_snapshot: !new_segment,
            base_path,
            next_part,
            rollover_pending: false,
        });
        if let Some(net) = self.net.as_mut() {
            net.session_mut().set_demo_capture(true);
        }
        self.push_console_path_line(
            format!("^2recording to {}.", destination.display()),
            destination.clone(),
        );
        true
    }

    /// The decoder reports Gamestate before DemoMessage, but the latter owns
    /// the original packet bytes and sequence. Defer the split until then.
    pub(in crate::app) fn mark_demo_recording_map_change(&mut self) {
        if let Some(recording) = self.demo_recording.as_mut() {
            recording.rollover_pending = true;
        }
    }

    fn rollover_demo_recording(&mut self, sequence: i32, payload: Vec<u8>) {
        let Some(recording) = self.demo_recording.as_ref() else { return };
        let base_path = recording.base_path.clone();
        let mut part = recording.next_part;
        if !self.finish_demo_recording() {
            return;
        }
        loop {
            let destination = demo_segment_path(&base_path, part);
            if !destination.exists() {
                // The gamestate packet itself is the first record of this
                // segment. Never prepend a synthetic gamestate from a decoder
                // that may already have consumed newer network packets.
                if self.begin_demo_recording(
                    destination, base_path, part.saturating_add(1), true,
                    Some((sequence, payload)),
                ) {
                    self.push_console_line("^3Map changed; recording continues in the new file.");
                }
                return;
            }
            let Some(next) = part.checked_add(1) else {
                self.push_console_line("^1No free demo segment number; recording stopped.");
                return;
            };
            part = next;
        }
    }

    pub(in crate::app) fn record_demo_message(
        &mut self,
        sequence: i32,
        payload: Vec<u8>,
        full_snapshot: bool,
    ) {
        if self.demo_recording.as_ref().is_some_and(|recording| recording.rollover_pending) {
            self.rollover_demo_recording(sequence, payload);
            return;
        }
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
        self.finish_demo_recording();
    }

    /// Returns false if the previous segment could not be finalized; do not
    /// silently continue a broken capture into another file.
    fn finish_demo_recording(&mut self) -> bool {
        let Some(mut recording) = self.demo_recording.take() else {
            self.push_console_line("^3Not recording a demo.".to_owned());
            return false;
        };
        if let Some(net) = self.net.as_mut() {
            net.session_mut().set_demo_capture(false);
        }
        let result = demo::write_end(&mut recording.file).and_then(|()| recording.file.flush());
        match result {
            Ok(()) => {
                if recording.path.extension().is_some_and(|ext| ext.eq_ignore_ascii_case("dm_15") || ext.eq_ignore_ascii_case("dm_16")) {
                    // Rescan the JK2 Demo archive next time it is opened.
                    self.jk2_demo_browser.reset();
                }
                self.push_console_path_line(
                    format!("^2Stopped demo: {}", recording.path.display()),
                    recording.path.clone(),
                );
                true
            }
            Err(error) => {
                self.push_console_path_line(
                    format!("^1Error finishing demo {}: {error}", recording.path.display()),
                    recording.path.clone(),
                );
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::demo_segment_path;
    use std::path::Path;

    #[test]
    fn rollover_preserves_folder_and_protocol_extension() {
        let source = Path::new("demos/custom/session.dm_26");
        assert_eq!(demo_segment_path(source, 2), Path::new("demos/custom/session_part002.dm_26"));
        assert_eq!(demo_segment_path(source, 12), Path::new("demos/custom/session_part012.dm_26"));
        let jk2 = Path::new("demos/demo0000.dm_16");
        assert_eq!(demo_segment_path(jk2, 2), Path::new("demos/demo0000_part002.dm_16"));
    }
}
