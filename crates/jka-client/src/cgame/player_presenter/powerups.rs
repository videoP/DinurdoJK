//! jaPRO cg_players.c: CG_PlayerFlag / CG_PlayerPowerups.
use super::{
    angles_to_axis, model_bolt_matrix_timed, openjk_vectoangles, transform_jka_model_point,
    transform_jka_model_vector, CosmeticDraw, Matrix3x4, PlayerFxRequest, PlayerModelAsset,
    PlayerPresenter, PresentedEntity, ET_NPC, ET_PLAYER,
};

const PW_QUAD: i32 = 1;
const PW_REDFLAG: i32 = 4;
const PW_BLUEFLAG: i32 = 5;
const PW_NEUTRALFLAG: i32 = 6;

fn flag_model(powerup: i32, gametype: i32, japro_server: bool) -> Option<&'static str> {
    // CG_RegisterGraphics gives jaPRO the ordinary flag models even in CTY,
    // allowing its server to switch gametypes without re-registering media.
    let cty = gametype == 9 && !japro_server;
    match powerup {
        PW_REDFLAG if cty => Some("models/flags/r_flag_ysal.md3"),
        PW_BLUEFLAG if cty => Some("models/flags/b_flag_ysal.md3"),
        PW_REDFLAG => Some("models/flags/r_flag.md3"),
        PW_BLUEFLAG => Some("models/flags/b_flag.md3"),
        PW_NEUTRALFLAG if gametype == 7 => Some("models/flags/n_flag.md3"),
        PW_NEUTRALFLAG if japro_server && matches!(gametype, 0 | 6 | 8) => {
            Some("models/flags/n_flag.md3")
        }
        _ => None,
    }
}

fn flag_transform(
    lumbar: &Matrix3x4,
    player_axis: [[f32; 3]; 3],
    player_origin: [f32; 3],
    lerp_angles: [f32; 3],
) -> ([f32; 3], [[f32; 3]; 3]) {
    let mut origin = transform_jka_model_point(
        [lumbar[0][3], lumbar[1][3], lumbar[2][3]],
        player_axis,
        player_origin,
    );
    // G2API_GetBoltMatrix adds a 270-degree yaw to the internal matrix.
    // Its POSITIVE_X is therefore negative column 1 of our low bolt matrix.
    let forward =
        transform_jka_model_vector([-lumbar[0][1], -lumbar[1][1], -lumbar[2][1]], player_axis);
    let lumbar_yaw = openjk_vectoangles(forward)[1];
    let left = angles_to_axis([0.0, lerp_angles[1], 0.0])[1];
    origin[2] -= 12.0;
    for i in 0..3 {
        origin[i] -= left[i] * 8.0;
    }
    let mut angles = [
        -lerp_angles[0] * 0.5 - 30.0,
        lumbar_yaw + 270.0,
        lerp_angles[2],
    ];
    let axis = angles_to_axis(angles);
    for i in 0..3 {
        origin[i] += axis[0][i] * 24.0;
    }
    // The extra roll applies to the model, after calculating its origin.
    angles[2] += 20.0;
    let axis = angles_to_axis(angles).map(|basis| basis.map(|v| v * 0.5));
    (origin, axis)
}

impl PlayerPresenter {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn queue_player_powerups(
        &mut self,
        entity: &PresentedEntity,
        gametype: i32,
        model: &PlayerModelAsset,
        pose: &[Matrix3x4],
        axis: [[f32; 3]; 3],
        origin: [f32; 3],
        submit_geometry: bool,
    ) -> Result<(), String> {
        // Body-queue copies do not run CG_PlayerPowerups.
        if !matches!(entity.entity_type, ET_PLAYER | ET_NPC) {
            return Ok(());
        }
        let powerups = entity.state.field_i32("powerups").unwrap_or(0);
        let colors = [
            (PW_QUAD, [0.2, 0.2, 1.0]),
            (PW_REDFLAG, [1.0, 0.2, 0.2]),
            (PW_BLUEFLAG, [0.2, 0.2, 1.0]),
            (PW_NEUTRALFLAG, [0.2, 1.0, 0.2]),
        ];
        // jaPRO adds these lights even for the first-person player whose flag
        // model is hidden. Model availability does not gate the carrier light.
        for (bit, rgb) in colors {
            if powerups & (1 << bit) != 0 {
                self.fx_requests.push(PlayerFxRequest::PowerupLight {
                    origin: entity.origin,
                    rgb,
                });
            }
        }
        if !submit_geometry
            || powerups & ((1 << PW_REDFLAG) | (1 << PW_BLUEFLAG) | (1 << PW_NEUTRALFLAG)) == 0
        {
            return Ok(());
        }
        let Some(lumbar) =
            model_bolt_matrix_timed(&mut self.perf, &model.glm, &model.gla, pose, "lower_lumbar")?
        else {
            return Ok(());
        };
        let (origin, axis) = flag_transform(&lumbar, axis, origin, entity.angles);
        let alpha = if i32::from(entity.number) == self.viewer_client {
            self.japro.third_person_flag_alpha
        } else {
            1.0
        };
        for bit in [PW_REDFLAG, PW_BLUEFLAG, PW_NEUTRALFLAG] {
            if powerups & (1 << bit) == 0 {
                continue;
            }
            if let Some(model) = flag_model(bit, gametype, self.viewer_style.japro) {
                self.cosmetic_draws.push(CosmeticDraw {
                    face_camera: None,
                    entity_num: entity.number,
                    model: model.to_owned(),
                    origin,
                    axis,
                    rgba: [1.0, 1.0, 1.0, alpha],
                    custom_shader: None,
                    skin_pose:None,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn siege_flags_use_existing_carrier_models_on_stock_and_japro_servers() {
        for japro in [false, true] {
            assert_eq!(flag_model(PW_REDFLAG, 7, japro), Some("models/flags/r_flag.md3"));
            assert_eq!(flag_model(PW_BLUEFLAG, 7, japro), Some("models/flags/b_flag.md3"));
            assert_eq!(flag_model(PW_NEUTRALFLAG, 7, japro), Some("models/flags/n_flag.md3"));
        }
        assert_eq!(flag_model(PW_REDFLAG, 9, false), Some("models/flags/r_flag_ysal.md3"));
    }

    #[test]
    #[ignore = "requires JKA_TEST_BASE with stock PK3s"]
    fn carried_flag_renders_from_stock_lumbar_and_hides_in_first_person() {
        use jka_assets::pk3::AssetSearchPath;
        use jka_protocol::gamestate::{EntityState, ENTITY_FIELDS};
        let base = std::env::var_os("JKA_TEST_BASE").expect("set JKA_TEST_BASE");
        let assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let mut presenter = PlayerPresenter::new(assets, false).unwrap();
        let model = presenter
            .load_model(&crate::cgame::ClientInfo::solo_model("kyle/default"))
            .unwrap();
        let pose = super::super::Ghoul2Animator::new(&model.gla)
            .evaluate_pose_openjk_root(&model.gla, 1000)
            .unwrap();
        let mut state = EntityState {
            number: 3,
            fields: [0; ENTITY_FIELDS.len()],
        };
        let slot = ENTITY_FIELDS
            .iter()
            .position(|(name, _)| *name == "powerups")
            .unwrap();
        state.fields[slot] = (1 << PW_REDFLAG) | (1 << PW_BLUEFLAG);
        let entity = PresentedEntity {
            number: 3,
            entity_type: ET_PLAYER,
            origin: [100.0, 200.0, 30.0],
            angles: [0.0, 90.0, 0.0],
            state,
        };
        presenter.viewer_client = 3;
        presenter.japro.third_person_flag_alpha = 0.25;
        let axis = angles_to_axis(entity.angles);
        presenter
            .queue_player_powerups(&entity, 8, &model, &pose, axis, entity.origin, false)
            .unwrap();
        assert!(
            presenter.drain_cosmetic_draws().is_empty(),
            "first-person flags must stay hidden"
        );
        assert_eq!(
            presenter.drain_fx_requests().len(),
            2,
            "first-person carriers still emit lights"
        );

        presenter
            .queue_player_powerups(&entity, 8, &model, &pose, axis, entity.origin, true)
            .unwrap();
        let flags = presenter.drain_cosmetic_draws();
        assert_eq!(
            flags.len(),
            2,
            "stock Kyle must resolve its lower_lumbar bolt"
        );
        assert_eq!(flags[0].rgba[3], 0.25);
        assert_eq!(flags[1].model, "models/flags/b_flag.md3");
        let assets = AssetSearchPath::open(std::path::Path::new(&base)).unwrap();
        let mut md3 = crate::cgame::entity_presenter::EntityPresenter::new(assets, false).unwrap();
        let surfaces = md3.present_cosmetics(flags);
        assert!(
            !surfaces.is_empty(),
            "carried flag MD3s must produce renderable surfaces"
        );
        assert!(surfaces.iter().all(|surface| surface
            .vertices
            .iter()
            .all(|v| v.position.iter().all(|p| p.is_finite()))));
        assert!(surfaces
            .iter()
            .all(|surface| surface.alpha_mode == super::super::DynamicModelAlphaMode::Blend));

        presenter.viewer_client = 7;
        presenter
            .queue_player_powerups(&entity, 8, &model, &pose, axis, entity.origin, true)
            .unwrap();
        assert!(
            presenter
                .drain_cosmetic_draws()
                .iter()
                .all(|flag| flag.rgba[3] == 1.0),
            "flag alpha affects only the viewer"
        );
    }

    #[test]
    fn flag_media_matches_japro_and_stock_cty_registration() {
        assert_eq!(
            flag_model(PW_REDFLAG, 8, false),
            Some("models/flags/r_flag.md3")
        );
        assert_eq!(
            flag_model(PW_BLUEFLAG, 9, false),
            Some("models/flags/b_flag_ysal.md3")
        );
        assert_eq!(
            flag_model(PW_REDFLAG, 9, true),
            Some("models/flags/r_flag.md3")
        );
        assert_eq!(
            flag_model(PW_NEUTRALFLAG, 8, true),
            Some("models/flags/n_flag.md3")
        );
        assert_eq!(flag_model(PW_NEUTRALFLAG, 8, false), None);
        assert_eq!(flag_model(PW_NEUTRALFLAG, 9, true), None);
    }

    #[test]
    fn lumbar_attachment_uses_public_bolt_axes_offsets_and_half_scale() {
        let lumbar = [
            [1.0, 0.0, 0.0, 10.0],
            [0.0, 1.0, 0.0, 20.0],
            [0.0, 0.0, 1.0, 30.0],
        ];
        let (origin, axis) = flag_transform(&lumbar, angles_to_axis([0.0; 3]), [0.0; 3], [0.0; 3]);
        let expected = [10.0 - 24.0 * 30.0_f32.to_radians().cos(), 12.0, 30.0];
        for i in 0..3 {
            assert!((origin[i] - expected[i]).abs() < 1e-4, "{origin:?}");
            let length = axis[i].iter().map(|v| v * v).sum::<f32>().sqrt();
            assert!((length - 0.5).abs() < 1e-5);
        }
        let expected_axis = angles_to_axis([-30.0, 180.0, 20.0]);
        for i in 0..3 {
            for j in 0..3 {
                assert!((axis[i][j] - expected_axis[i][j] * 0.5).abs() < 1e-5);
            }
        }
    }
}
