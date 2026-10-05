//! Physics.
use crate::app::egui_settings::{
    config, quality_table_row, segmented_row, theme, ui, App, PHYS_CCD, PHYS_CLIENT_ENABLED,
    PHYS_CLOTH, PHYS_CLOTH_AIR, PHYS_CLOTH_ANIMATION, PHYS_CLOTH_BODY_COLLISION,
    PHYS_CLOTH_CLEARANCE, PHYS_CLOTH_TURN, PHYS_CLOTH_WIND, PHYS_DEBRIS, PHYS_DEBRIS_LIFETIME,
    PHYS_DEBRIS_MAX, PHYS_DEBUG_DRAW, PHYS_DISMEMBERMENT, PHYS_DISMEMBER_LIFETIME,
    PHYS_DISMEMBER_MAX, PHYS_EXPLOSION_IMPULSES, PHYS_FORCE_IMPULSES, PHYS_JIGGLE,
    PHYS_JIGGLE_BREAST, PHYS_JIGGLE_DAMPING, PHYS_JIGGLE_GLUTE, PHYS_JIGGLE_GLUTE_LIFT,
    PHYS_JIGGLE_JP_AIR_DRAG, PHYS_JIGGLE_JP_DRAG, PHYS_JIGGLE_JP_GRAVITY,
    PHYS_JIGGLE_JP_SOFTEN, PHYS_JIGGLE_JP_STIFFNESS, PHYS_JIGGLE_JP_STRETCH, PHYS_JIGGLE_SOLVER,
    PHYS_JIGGLE_STIFFNESS, PHYS_JIGGLE_STRENGTH, PHYS_MAX_SUBSTEPS, PHYS_PLAYER_PUSH, PHYS_PROPS,
    PHYS_PROP_MAX, PHYS_RAGDOLLS, PHYS_RAGDOLL_LIFETIME, PHYS_RAGDOLL_MAX,
    PHYS_RAGDOLL_SELF_COLLISION, PHYS_RATE, PHYS_SLEEPING, PHYS_STATS, PHYS_WEAPON_IMPULSES,
};

impl App {
    pub(in crate::app::egui_settings) fn egui_physics(&mut self, ui: &mut egui::Ui) {
        theme::banner(
            ui,
            "RAPIER IS CLIENT-ONLY VISUAL PHYSICS — OPENJK/JKA MOVEMENT AND SERVER SNAPSHOTS STAY AUTHORITATIVE.",
            theme::TEXT_FAINT,
        );

        theme::section(
            ui,
            "PLAYER MOVEMENT",
            "Gameplay, not visuals: the fixed tick the local player is simulated at.",
        );
        theme::row(
            ui,
            "Player movement rate",
            "Fixed OpenJK/JKA player movement tick. Stock Jedi Academy runs 125 Hz; \
             changing it changes movement feel, including strafe jump behaviour. \
             This is separate from client-side Rapier visual physics.",
            theme::Reset::Video(ui::VIDEO_ROW_PHYSICS_FPS),
            |ui| {
                let mut fps = config::physics_fps_from_msec(self.video.physics_msec);
                if ui
                    .add(
                        egui::DragValue::new(&mut fps)
                            .range(20..=1000)
                            .speed(1.0)
                            .suffix(" Hz")
                            .update_while_editing(false),
                    )
                    .changed()
                {
                    let msec =
                        config::physics_msec_from_fps(&fps.to_string(), self.video.physics_msec);
                    self.set_physics_msec(msec);
                }
            },
        );

        theme::section(
            ui,
            "SIMULATION",
            "Independent from authoritative player movement and server snapshots.",
        );
        self.egui_physics_toggle(
            ui,
            "Client physics",
            "Master switch for client-side visual physics. Rapier handles rigid/ragdoll simulation, \
             while cloth and jiggle use dedicated solvers. It never replaces authoritative JKA movement.",
            PHYS_CLIENT_ENABLED,
            self.video.client_physics,
        );

        const RATES: [(u32, &str); 4] = [
            (30, "30 Hz"),
            (60, "60 Hz"),
            (120, "120 Hz"),
            (240, "240 Hz"),
        ];
        if let Some(rate) = segmented_row(
            ui,
            "Simulation rate",
            "Fixed target timestep shared by client visual-physics solvers. 60 Hz is the baseline; \
             higher rates improve fast secondary motion and contacts at additional CPU cost.",
            theme::Reset::Physics(PHYS_RATE),
            self.video.client_physics_hz,
            &RATES,
        ) {
            self.video.client_physics_hz = rate;
            self.physics_menu_changed();
        }

        const SUBSTEPS: [(u32, &str); 4] = [(1, "1"), (2, "2"), (4, "4"), (8, "8")];
        if let Some(steps) = quality_table_row(
            ui,
            "Catch-up steps",
            "Maximum fixed steps the visual simulation may execute after a slow frame. This caps \
             spiral-of-death behavior without changing the fixed timestep.",
            theme::Reset::Physics(PHYS_MAX_SUBSTEPS),
            self.video.client_physics_max_substeps,
            &SUBSTEPS,
            2,
        ) {
            self.video.client_physics_max_substeps = SUBSTEPS[steps].0;
            self.physics_menu_changed();
        }

        self.egui_physics_toggle(
            ui,
            "Continuous collision detection",
            "Enables Rapier CCD for fast-moving visual bodies that would otherwise tunnel through \
             thin map geometry. Individual bodies can still opt out later.",
            PHYS_CCD,
            self.video.client_physics_ccd,
        );
        self.egui_physics_toggle(
            ui,
            "Sleeping",
            "Lets inactive rigid bodies sleep so settled ragdolls, props and debris stop consuming \
             solver time until disturbed.",
            PHYS_SLEEPING,
            self.video.client_physics_sleeping,
        );

        theme::section(
            ui,
            "RAGDOLLS",
            "Death and knockdown presentation driven by a local articulated body.",
        );
        self.egui_physics_toggle(
            ui,
            "Ragdolls",
            "Allow corpses, Force Grip victims and short living knockback reactions to use the client-only \
             articulated Rapier presentation while server movement remains authoritative.",
            PHYS_RAGDOLLS,
            self.video.ragdolls,
        );
        const RAGDOLL_MAX: [(u32, &str); 5] =
            [(2, "2"), (4, "4"), (8, "8"), (16, "16"), (32, "32")];
        if let Some(index) = quality_table_row(
            ui,
            "Active ragdolls",
            "Maximum articulated ragdolls kept in the local physics world before the oldest are \
             retired to a cheaper static/animated corpse path.",
            theme::Reset::Physics(PHYS_RAGDOLL_MAX),
            self.video.ragdoll_max,
            &RAGDOLL_MAX,
            2,
        ) {
            self.video.ragdoll_max = RAGDOLL_MAX[index].0;
            self.physics_menu_changed();
        }
        const RAGDOLL_LIFE: [(u32, &str); 5] = [
            (5, "5 s"),
            (10, "10 s"),
            (20, "20 s"),
            (30, "30 s"),
            (60, "60 s"),
        ];
        let ragdoll_life = self.video.ragdoll_lifetime.round().clamp(1.0, 300.0) as u32;
        if let Some(index) = quality_table_row(
            ui,
            "Ragdoll lifetime",
            "How long a client ragdoll remains actively simulated before its bodies are put to \
             sleep. The solved pose remains visible; server entity lifetime is unchanged.",
            theme::Reset::Physics(PHYS_RAGDOLL_LIFETIME),
            ragdoll_life,
            &RAGDOLL_LIFE,
            2,
        ) {
            self.video.ragdoll_lifetime = RAGDOLL_LIFE[index].0 as f32;
            self.physics_menu_changed();
        }
        self.egui_physics_toggle(
            ui,
            "Ragdoll self-collision",
            "Allows limbs on the same ragdoll to collide. This can look more physical but increases \
             solver work and can make constrained skeletons less stable.",
            PHYS_RAGDOLL_SELF_COLLISION,
            self.video.ragdoll_self_collision,
        );
        self.egui_physics_toggle(
            ui,
            "Weapon knockback ragdolls",
            "Let authoritative saber/projectile impacts temporarily hand a living victim's limbs/head \
             to Rapier when the corresponding trajectory shows real knockback. The thorax stays attached \
             to the server-owned player transform.",
            PHYS_WEAPON_IMPULSES,
            self.video.physics_weapon_impulses,
        );
        self.egui_physics_toggle(
            ui,
            "Explosion knockback ragdolls",
            "Let explosion impact events open a short candidate window for nearby players; only players \
             whose authoritative trajectory actually receives a matching impulse are ragdolled.",
            PHYS_EXPLOSION_IMPULSES,
            self.video.physics_explosion_impulses,
        );
        self.egui_physics_toggle(
            ui,
            "Force push/pull ragdolls",
            "Use networked Force Push/Pull presentation plus the victim's authoritative trajectory change \
             to drive temporary torso-anchored impulse ragdolls. Force Grip keeps its separate neck anchor.",
            PHYS_FORCE_IMPULSES,
            self.video.physics_force_impulses,
        );

        theme::section(
            ui,
            "SOFT TISSUE (EXPERIMENTAL)",
            "Profile-driven post-skin secondary motion for stock _humanoid player meshes.",
        );
        self.egui_physics_toggle(
            ui,
            "Jiggle physics",
            "Enable _humanoid soft-tissue secondary motion. Chest/glute regions are auto-detected; \
             model.jiggle overrides Auto. GPU skinning promotes only the interior soft region; a stock-topology guard band hides the transition, with extra refinement reserved for the strongest core.",
            PHYS_JIGGLE,
            self.video.jiggle_physics,
        );
        const JIGGLE_SOLVER: [(u8, &str); 2] = [(0, "KawaiiPhysics"), (1, "JigglePhysics")];
        if let Some(mode) = segmented_row(
            ui,
            "Solver",
            "Choose the secondary-motion algorithm. KawaiiPhysics uses its fixed-step point integration; JigglePhysics ports naelstrof/JigglePhysics' relativistic Verlet + angle/length constraints for a Motionless Root virtual child.",
            theme::Reset::Physics(PHYS_JIGGLE_SOLVER),
            self.video.jiggle_solver,
            &JIGGLE_SOLVER,
        ) {
            let _ = self.set_console_cvar("r_jiggleSolver", &mode.to_string());
        }

        for (label, tip, field, current, min, max, cvar) in [
            ("Overall motion", "Global multiplier for all soft-tissue displacement.", PHYS_JIGGLE_STRENGTH, self.video.jiggle_strength, 0.0, 2.0, "r_jiggleStrength"),
            ("Breast motion", "Multiplier for chest-region displacement only.", PHYS_JIGGLE_BREAST, self.video.jiggle_breast_strength, 0.0, 2.0, "r_jiggleBreastStrength"),
            ("Glute motion", "Multiplier for glute-region displacement only.", PHYS_JIGGLE_GLUTE, self.video.jiggle_glute_strength, 0.0, 2.0, "r_jiggleGluteStrength"),
            ("Glute lower trim", "Trim the lower glute/upper-thigh falloff without rebuilding the model. Positive values keep motion higher on the glute; negative values include more upper thigh.", PHYS_JIGGLE_GLUTE_LIFT, self.video.jiggle_glute_lift, -0.4, 0.6, "r_jiggleGluteLift"),
        ] {
            theme::row(ui, label, tip, theme::Reset::Physics(field), |ui| {
                let mut value = current;
                let readout = format!("{value:.2}");
                if theme::slider(ui, &mut value, min..=max, &readout) {
                    let _ = self.set_console_cvar(cvar, &value.to_string());
                }
            });
        }

        if self.video.jiggle_solver == 0 {
            for (label, tip, field, current, cvar) in [
                ("Stiffness", "KawaiiPhysics stiffness multiplier. Higher values pull the virtual soft point toward the animated pose faster.", PHYS_JIGGLE_STIFFNESS, self.video.jiggle_stiffness, "r_jiggleStiffness"),
                ("Damping", "KawaiiPhysics damping multiplier. Higher values remove secondary velocity faster.", PHYS_JIGGLE_DAMPING, self.video.jiggle_damping, "r_jiggleDamping"),
            ] {
                theme::row(ui, label, tip, theme::Reset::Physics(field), |ui| {
                    let mut value = current;
                    let readout = format!("{value:.2}");
                    if theme::slider(ui, &mut value, 0.0..=3.0, &readout) {
                        let _ = self.set_console_cvar(cvar, &value.to_string());
                    }
                });
            }
        } else {
            for (label, tip, field, current, min, max, cvar) in [
                ("Stiffness", "JigglePhysics Stiffness: target-angle constraint strength (0..1).", PHYS_JIGGLE_JP_STIFFNESS, self.video.jiggle_jp_stiffness, 0.0, 1.0, "r_jiggleJPStiffness"),
                ("Drag", "JigglePhysics mechanical drag: damps motion relative to the parent/root (0..1).", PHYS_JIGGLE_JP_DRAG, self.video.jiggle_jp_drag, 0.0, 1.0, "r_jiggleJPDrag"),
                ("Air drag", "JigglePhysics air drag: damps inherited world-space/root motion separately from local oscillation (0..1).", PHYS_JIGGLE_JP_AIR_DRAG, self.video.jiggle_jp_air_drag, 0.0, 1.0, "r_jiggleJPAirDrag"),
                ("Stretch", "JigglePhysics Stretch: 0 enforces authored length fully; 1 makes length correction track Stiffness.", PHYS_JIGGLE_JP_STRETCH, self.video.jiggle_jp_stretch, 0.0, 1.0, "r_jiggleJPStretch"),
                ("Soften", "JigglePhysics Soften: reduces stiffness close to the animated/rest target so small oscillation survives.", PHYS_JIGGLE_JP_SOFTEN, self.video.jiggle_jp_soften, 0.0, 1.0, "r_jiggleJPSoften"),
                ("Gravity", "JigglePhysics gravity multiplier. 0 is the body-tissue default here; increase it for deliberate droop.", PHYS_JIGGLE_JP_GRAVITY, self.video.jiggle_jp_gravity, 0.0, 2.0, "r_jiggleJPGravity"),
            ] {
                theme::row(ui, label, tip, theme::Reset::Physics(field), |ui| {
                    let mut value = current;
                    let readout = format!("{value:.2}");
                    if theme::slider(ui, &mut value, min..=max, &readout) {
                        let _ = self.set_console_cvar(cvar, &value.to_string());
                    }
                });
            }
        }

        theme::section(
            ui,
            "DISMEMBERMENT",
            "OpenJK server-authored severing with Ghoul2 stump caps and one Rapier body per detached part.",
        );
        const DISMEMBERMENT: [(u8, &str); 3] = [(0, "Off"), (1, "Limbs"), (2, "Full")];
        if let Some(mode) = segmented_row(
            ui,
            "Dismemberment",
            "OpenJK cg_dismember semantics. Limbs hides head/waist severing; Full permits every server-authored part. \
             This never invents a client-side saber hit or changes gameplay.",
            theme::Reset::Physics(PHYS_DISMEMBERMENT),
            self.video.dismemberment,
            &DISMEMBERMENT,
        ) {
            self.video.dismemberment = mode;
            self.physics_menu_changed();
        }
        const DISMEMBER_MAX: [(u32, &str); 5] =
            [(8, "8"), (16, "16"), (24, "24"), (48, "48"), (96, "96")];
        if let Some(index) = quality_table_row(
            ui,
            "Detached limb budget",
            "Maximum detached rigid bodies retained locally. The oldest is retired first when the budget is exceeded.",
            theme::Reset::Physics(PHYS_DISMEMBER_MAX),
            self.video.dismember_max,
            &DISMEMBER_MAX,
            2,
        ) {
            self.video.dismember_max = DISMEMBER_MAX[index].0;
            self.physics_menu_changed();
        }
        const DISMEMBER_LIFE: [(u32, &str); 5] = [
            (5, "5 s"),
            (10, "10 s"),
            (16, "16 s"),
            (30, "30 s"),
            (60, "60 s"),
        ];
        let dismember_life = self.video.dismember_lifetime.round().clamp(1.0, 300.0) as u32;
        if let Some(index) = quality_table_row(
            ui,
            "Detached limb lifetime",
            "Maximum local Rapier lifetime for a detached part. Server entity lifetime is still authoritative for visibility.",
            theme::Reset::Physics(PHYS_DISMEMBER_LIFETIME),
            dismember_life,
            &DISMEMBER_LIFE,
            2,
        ) {
            self.video.dismember_lifetime = DISMEMBER_LIFE[index].0 as f32;
            self.physics_menu_changed();
        }

        theme::section(
            ui,
            "CLOTH (EXPERIMENTAL)",
            "Secondary garment motion driven by movement, turning and animation.",
        );
        self.egui_physics_toggle(
            ui,
            "Cape / robe cloth",
            "Add cloth sway and momentum to cape, cloak and robe surfaces while preserving \
             their animated shape.",
            PHYS_CLOTH,
            self.video.cloth_physics,
        );
        self.egui_physics_toggle(
            ui,
            "Cloth body collision",
            "Keep cloth from moving through the animated head, torso, arms and legs. \
             Collision adapts to each garment's authored fit.",
            PHYS_CLOTH_BODY_COLLISION,
            self.video.cloth_body_collision,
        );

        for (label, tip, field, current, maximum, cvar) in [
            ("Body clearance", "Additional gap between the fabric and the body, in JKA units.",
                PHYS_CLOTH_CLEARANCE, self.video.cloth_body_clearance, 4.0, "r_clothBodyClearance"),
            ("Air resistance", "Air pressure on moving fabric. Higher values make running and stopping pull the garment more strongly.",
                PHYS_CLOTH_AIR, self.video.cloth_air_resistance, 4.0, "r_clothAirResistance"),
            ("Turning response", "How strongly fabric lags and swings as the character turns.",
                PHYS_CLOTH_TURN, self.video.cloth_turn_response, 4.0, "r_clothTurnResponse"),
            ("Animation influence", "How closely free fabric follows skeletal animation. At 0 it follows its sewn attachments; at 1 it follows the full authored pose.",
                PHYS_CLOTH_ANIMATION, self.video.cloth_animation_influence, 1.0, "r_clothAnimationInfluence"),
        ] {
            theme::row(ui, label, tip, theme::Reset::Physics(field), |ui| {
                let mut value = current;
                let readout = format!("{value:.2}");
                if theme::slider(ui, &mut value, 0.0..=maximum, &readout) {
                    let _ = self.set_console_cvar(cvar, &value.to_string());
                }
            });
        }
        self.egui_physics_toggle(
            ui,
            "Weather wind",
            "Apply the shared weather wind direction and gusts to cloth.",
            PHYS_CLOTH_WIND,
            self.video.cloth_wind,
        );

        theme::section(
            ui,
            "PROPS",
            "Client-only dynamic objects layered over the server world.",
        );
        self.egui_physics_toggle(
            ui,
            "Dynamic props",
            "Enable Rapier simulation for explicitly promoted visual props. Server-owned movers and \
             gameplay entities remain snapshot-driven.",
            PHYS_PROPS,
            self.video.physics_props,
        );
        const PROP_MAX: [(u32, &str); 5] = [
            (32, "32"),
            (64, "64"),
            (96, "96"),
            (192, "192"),
            (384, "384"),
        ];
        if let Some(index) = quality_table_row(
            ui,
            "Active props",
            "Budget for simultaneously simulated loose visual props.",
            theme::Reset::Physics(PHYS_PROP_MAX),
            self.video.physics_prop_max,
            &PROP_MAX,
            2,
        ) {
            self.video.physics_prop_max = PROP_MAX[index].0;
            self.physics_menu_changed();
        }

        theme::section(
            ui,
            "DEBRIS",
            "Short-lived physical fragments spawned from presentation events.",
        );
        self.egui_physics_toggle(
            ui,
            "Physical debris",
            "Use rigid bodies for client-spawned fragments instead of purely ballistic particles.",
            PHYS_DEBRIS,
            self.video.physics_debris,
        );
        const DEBRIS_MAX: [(u32, &str); 5] = [
            (64, "64"),
            (128, "128"),
            (192, "192"),
            (384, "384"),
            (768, "768"),
        ];
        if let Some(index) = quality_table_row(
            ui,
            "Debris pieces",
            "Maximum number of short-lived debris rigid bodies in the local simulation.",
            theme::Reset::Physics(PHYS_DEBRIS_MAX),
            self.video.physics_debris_max,
            &DEBRIS_MAX,
            2,
        ) {
            self.video.physics_debris_max = DEBRIS_MAX[index].0;
            self.physics_menu_changed();
        }
        const DEBRIS_LIFE: [(u32, &str); 5] = [
            (2, "2 s"),
            (5, "5 s"),
            (10, "10 s"),
            (20, "20 s"),
            (30, "30 s"),
        ];
        let debris_life = self.video.physics_debris_lifetime.round().clamp(1.0, 300.0) as u32;
        if let Some(index) = quality_table_row(
            ui,
            "Debris lifetime",
            "How long physical debris remains before it is culled from the local physics world.",
            theme::Reset::Physics(PHYS_DEBRIS_LIFETIME),
            debris_life,
            &DEBRIS_LIFE,
            2,
        ) {
            self.video.physics_debris_lifetime = DEBRIS_LIFE[index].0 as f32;
            self.physics_menu_changed();
        }

        theme::section(
            ui,
            "PROP INTERACTION",
            "Future one-way contact between server-owned players and client-only props.",
        );
        self.egui_physics_toggle(
            ui,
            "Player pushes props",
            "Represent the local/server player capsule as kinematic input to Rapier so contact can \
             push visual props without letting those props alter JKA player movement.",
            PHYS_PLAYER_PUSH,
            self.video.physics_player_push,
        );

        theme::section(
            ui,
            "DEBUG",
            "Development-only diagnostics for the Rapier world.",
        );
        self.egui_physics_toggle(
            ui,
            "Physics diagnostics",
            "Print Rapier initialization, ragdoll candidate, rejection and spawn diagnostics to the console.",
            PHYS_DEBUG_DRAW,
            self.video.physics_debug_draw,
        );
        self.egui_physics_toggle(
            ui,
            "Physics stats",
            "Print Rapier body, collider, joint and fixed-step counters to the console once per second.",
            PHYS_STATS,
            self.video.physics_stats,
        );
    }

    pub(in crate::app::egui_settings) fn egui_physics_toggle(
        &mut self,
        ui: &mut egui::Ui,
        label: &str,
        tip: &str,
        field: u8,
        current: bool,
    ) {
        theme::row(ui, label, tip, theme::Reset::Physics(field), |ui| {
            if let Some(value) = theme::switch(ui, current) {
                self.set_physics_menu_bool(field, value);
            }
        });
    }

    pub(in crate::app::egui_settings) fn set_physics_menu_bool(&mut self, field: u8, value: bool) {
        match field {
            PHYS_CLIENT_ENABLED => self.video.client_physics = value,
            PHYS_CCD => self.video.client_physics_ccd = value,
            PHYS_SLEEPING => self.video.client_physics_sleeping = value,
            PHYS_RAGDOLLS => self.video.ragdolls = value,
            PHYS_RAGDOLL_SELF_COLLISION => self.video.ragdoll_self_collision = value,
            PHYS_JIGGLE => self.video.jiggle_physics = value,
            PHYS_CLOTH => self.video.cloth_physics = value,
            PHYS_CLOTH_BODY_COLLISION => self.video.cloth_body_collision = value,
            PHYS_CLOTH_WIND => self.video.cloth_wind = value,
            PHYS_PROPS => self.video.physics_props = value,
            PHYS_DEBRIS => self.video.physics_debris = value,
            PHYS_PLAYER_PUSH => self.video.physics_player_push = value,
            PHYS_WEAPON_IMPULSES => self.video.physics_weapon_impulses = value,
            PHYS_EXPLOSION_IMPULSES => self.video.physics_explosion_impulses = value,
            PHYS_FORCE_IMPULSES => self.video.physics_force_impulses = value,
            PHYS_DEBUG_DRAW => self.video.physics_debug_draw = value,
            PHYS_STATS => self.video.physics_stats = value,
            _ => return,
        }
        self.physics_menu_changed();
    }
}
