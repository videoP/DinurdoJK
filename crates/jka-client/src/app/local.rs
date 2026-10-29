//! Local.
use crate::app::App;

impl App {
    pub fn queue_startup_commands(&mut self, commands: Vec<String>) {
        self.startup_commands = commands;
    }

    pub fn configure_startup_demo(&mut self, path: std::path::PathBuf) {
        self.startup_demo_file = Some(path);
    }

    pub(in crate::app) fn toggle_noclip(&mut self) {
        let result = if let Some(server) = &mut self.local_server {
            server.toggle_noclip()
        } else {
            Err("NOCLIP: NO LOCAL PLAYER".into())
        };

        match result {
            Ok(true) => {
                self.console_status =
                    "NOCLIP ON  |  MOUSE1/MOUSE2 = 10X SPEED EACH  |  BOTH = 100X".into();
                self.push_console_line(format!("^2{}", self.console_status));
                self.push_local_snapshot(true);
            }
            Ok(false) => {
                self.console_status = "NOCLIP OFF".into();
                self.push_console_line(format!("^3{}", self.console_status));
                self.push_local_snapshot(true);
            }
            Err(error) => {
                self.console_status = error;
                self.push_console_line(format!("^1{}", self.console_status));
            }
        }
        self.update_solo_player_view_and_presentation();
        self.publish_snapshot();
        self.publish_ui();
    }

    /// Solo equivalent of jaPRO coordinate amTele. Unlike setviewpos,
    /// amTele's server handlers parse coordinates and yaw as integers and
    /// do not accept a pitch in coordinate form.
    pub(in crate::app) fn local_amtele(&mut self, arguments: &[&str]) {
        if !(3..=4).contains(&arguments.len()) {
            self.push_console_line("usage: amTele x y z [yaw]".to_owned());
            return;
        }
        let values: Option<Vec<i32>> = arguments.iter().map(|text| text.parse::<i32>().ok()).collect();
        let Some(values) = values else {
            self.push_console_line("^1amTele:^7 coordinates must be integers".to_owned());
            return;
        };
        let origin = [values[0] as f32, values[1] as f32, values[2] as f32];
        let angles = [0.0, values.get(3).copied().unwrap_or(0) as f32, 0.0];
        // This is the local implementation of amTele, not a setviewpos call.
        let result = self.local_server.as_mut().expect("checked by caller").teleport(origin, angles);
        match result {
            Ok(()) => {
                self.push_console_line(format!(
                    "^2amTele:^7 ({}, {}, {}) yaw {}",
                    values[0], values[1], values[2], values.get(3).copied().unwrap_or(0)
                ));
                self.push_local_snapshot(true);
                self.third_person_camera.reset();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
            }
            Err(error) => self.push_console_line(format!("^1amTele:^7 {error}")),
        }
    }

    /// OpenJK `Cmd_SetViewpos_f`: `setviewpos x y z yaw`, answered by the
    /// solo shim instead of a game module. DinurdoJK also accepts an optional
    /// trailing pitch so automated views are reproducible.
    pub(in crate::app) fn local_setviewpos(&mut self, arguments: &[&str]) {
        self.local_setviewpos_with_push(arguments, 0);
    }

    /// `push_speed` 0 is OpenJK's 400 u/s TeleportPlayer push; benchmarks pass 1
    /// (`setviewpos_exact`) so the camera stays on the requested view.
    pub(in crate::app) fn local_setviewpos_with_push(&mut self, arguments: &[&str], push_speed: i32) {
        if !(4..=5).contains(&arguments.len()) {
            self.push_console_line("usage: setviewpos x y z yaw [pitch]".to_owned());
            return;
        }
        // atof(): an unparsable token reads as 0, like the stock command.
        let value = |index: usize| {
            arguments
                .get(index)
                .and_then(|text| text.parse::<f32>().ok())
                .filter(|value| value.is_finite())
                .unwrap_or(0.0)
        };
        let origin = [value(0), value(1), value(2)];
        // Pitch into [-180, 180): PM_UpdateViewAngles clamps the unwrapped pitch to +/-16000 shorts (87.9 degrees), so
        // an equivalent angle such as -339 (= 21) otherwise lands looking straight up, depending on the cmd angles.
        let pitch = (value(4) + 180.0).rem_euclid(360.0) - 180.0;
        let angles = [pitch, value(3), 0.0];
        let result = self
            .local_server
            .as_mut()
            .expect("checked by caller")
            .teleport_with_push(origin, angles, push_speed);
        match result {
            Ok(()) => {
                println!(
                    "SETVIEWPOS: origin=({:.1} {:.1} {:.1}) yaw={:.1} pitch={:.1}",
                    origin[0], origin[1], origin[2], angles[1], angles[0]
                );
                self.push_local_snapshot(true);
                self.third_person_camera.reset();
                self.update_solo_player_view_and_presentation();
                self.publish_snapshot();
            }
            Err(error) => self.push_console_line(format!("^1setviewpos: {error}")),
        }
    }

    /// OpenJK `CG_Viewpos_f`: `(x y z) : yaw` of the rendered view origin.
    /// The pitch is appended so a view can be reproduced with setviewpos.
    pub(in crate::app) fn print_viewpos(&mut self) {
        let position = self.camera.position;
        let line = format!(
            "({} {} {}) : {} (pitch {})",
            position.x as i32,
            (-position.z) as i32,
            position.y as i32,
            self.camera.yaw.to_degrees().rem_euclid(360.0) as i32,
            (-self.camera.pitch.to_degrees()) as i32
        );
        self.push_console_line(line);
    }
}
