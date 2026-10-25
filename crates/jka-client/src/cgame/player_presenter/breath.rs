//! TaystJK CG_BreathPuffs (codemp/cgame/cg_players.c).
//! CGame owns the weather trigger; the Ghoul2 presenter owns per-centity timers
//! and attaches retail JKA EFX at the animated `*head_front` tag.
use crate::cgame::player_presenter::{
    ghoul2_model_scale, model_bolt_matrix_timed, normalize_vec3, transform_jka_model_point,
    transform_jka_model_vector, Matrix3x4, PlayerFxRequest, PlayerModelAsset, PlayerPresenter,
    PresentedEntity, EF_DEAD, ET_PLAYER,
};

const CONTENTS_WATER: i32 = 0x0000_0004;
const CONTENTS_LAVA: i32 = 0x0000_0002;
const CONTENTS_SLIME: i32 = 0x0002_0000;
const NORMAL_BREATH_MS: i32 = 3000;
const TALKING_BREATH_MS: i32 = 300;

fn next_breath_puff_time(now: i32, talking: bool) -> i32 {
    now.saturating_add(if talking { TALKING_BREATH_MS } else { NORMAL_BREATH_MS })
}

impl PlayerPresenter {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn queue_breath_puff(
        &mut self,
        entity: &PresentedEntity,
        model: &PlayerModelAsset,
        pose: &[Matrix3x4],
        player_axis: [[f32; 3]; 3],
        player_origin: [f32; 3],
        current_time: i32,
        submit_geometry: bool,
    ) -> Result<(), String> {
        // CG_G2PlayerAngles calls this for ET_PLAYER only, excluding corpses,
        // emplaced users and dead bodies. No NPC breathing in TaystJK MP.
        if entity.entity_type != ET_PLAYER
            || entity.state.field_i32("eFlags").unwrap_or(0) & EF_DEAD != 0
            || entity.state.field_i32("otherEntityNum2").unwrap_or(0) != 0
        {
            return Ok(());
        }

        let next = self.breath_puff_times.entry(entity.number).or_insert(0);
        if *next > current_time { return Ok(()); }
        // CG_BreathPuffs advances the timer BEFORE consulting the disable bit,
        // first-person camera, contents, weather flag, or *head_front tag.
        *next = next_breath_puff_time(current_time, self.breath_talking.contains(&entity.number));

        if self.japro.style_player & crate::japro_cg::style::DISABLE_BREATHING != 0
            || !submit_geometry
            || self.breath_view_contents & (CONTENTS_LAVA | CONTENTS_SLIME) != 0
            || self.breath_view_contents & CONTENTS_WATER == 0 && !self.cold_breath_effects
        {
            return Ok(());
        }

        // Native FX_PlayBoltedEffectID requires a real *head_front Ghoul2 bolt.
        // Never substitute *head_top (the smoke would exit the scalp).
        let Some(matrix) = model_bolt_matrix_timed(
            &mut self.perf, &model.glm, &model.gla, pose, "*head_front",
        )? else { return Ok(()); };
        let scale = ghoul2_model_scale(entity);
        let origin = transform_jka_model_point(
            [matrix[0][3] * scale, matrix[1][3] * scale, matrix[2][3] * scale],
            player_axis,
            player_origin,
        );
        // Ghoul2 internal (low) bolt matrix uses public +X=-low column 1,
        // +Y=low column 0, +Z=low column 2. Preserve the authored EFX frame.
        let mut axis = [[0.0; 3]; 3];
        for (index, column) in [1, 0, 2].into_iter().enumerate() {
            let sign = if index == 0 { -1.0 } else { 1.0 };
            axis[index] = normalize_vec3(transform_jka_model_vector(
                [matrix[0][column] * sign, matrix[1][column] * sign, matrix[2][column] * sign],
                player_axis,
            ));
        }
        let name = if self.breath_view_contents & CONTENTS_WATER != 0 {
            "effects/misc/waterbreath.efx"
        } else {
            "effects/misc/breath.efx"
        };
        self.fx_requests.push(PlayerFxRequest::Effect { name, origin, axis });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tayst_breath_intervals_and_contents_masks() {
        assert_eq!(NORMAL_BREATH_MS, 3000);
        assert_eq!(TALKING_BREATH_MS, 300);
        assert_eq!(next_breath_puff_time(500, false), 3500);
        assert_eq!(next_breath_puff_time(500, true), 800);
        assert_eq!(CONTENTS_WATER, 4);
        assert_eq!(CONTENTS_LAVA, 2);
        assert_eq!(CONTENTS_SLIME, 0x20000);
    }
}
