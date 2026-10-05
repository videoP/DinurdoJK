//! Local.
use crate::app::App;

impl App {
    pub fn queue_startup_commands(&mut self, commands: Vec<String>) {
        self.startup_commands = commands;
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

    /// OpenJK `Cmd_SetViewpos_f`: `setviewpos x y z yaw`, answered by the
    /// solo shim instead of a game module. DinurdoJK also accepts an optional
    /// trailing pitch so automated views are reproducible.
    pub(in crate::app) fn local_setviewpos(&mut self, arguments: &[&str]) {
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
        let angles = [value(4), value(3), 0.0];
        let result = self
            .local_server
            .as_mut()
            .expect("checked by caller")
            .teleport(origin, angles);
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
