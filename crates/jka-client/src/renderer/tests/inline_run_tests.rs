use super::*;

/// Runs the planner and returns each run as its list of visible positions.
fn plan(group_of: &[u32], continues: &[bool], groups: usize) -> Vec<Vec<u32>> {
    let mut scratch = InlineDrawScratch::default();
    plan_inline_runs(group_of, continues, groups, &mut scratch);
    scratch
        .runs
        .iter()
        .map(|&(first, count)| scratch.order[first as usize..(first + count) as usize].to_vec())
        .collect()
}

#[test]
fn same_stage_of_different_surfaces_merges_into_one_run() {
    // Three single-stage surfaces of one state, separated by another state.
    let runs = plan(&[0, 1, 0, 0], &[false; 4], 2);
    assert_eq!(runs, vec![vec![0, 2, 3], vec![1]]);
}

#[test]
fn stages_of_one_surface_never_draw_out_of_order() {
    // Y is a lone stage of state 1; X is stage 0 (state 0) then stage 1
    // (state 1). Merging state 1 at Y would draw X's second stage first.
    let runs = plan(&[1, 0, 1], &[false, false, true], 2);
    assert_eq!(runs, vec![vec![0], vec![1], vec![2]]);
}

#[test]
fn matching_stages_of_several_surfaces_stay_stage_major() {
    // Two surfaces, each state 0 then state 1: one run per stage.
    let runs = plan(&[0, 1, 0, 1], &[false, true, false, true], 2);
    assert_eq!(runs, vec![vec![0, 2], vec![1, 3]]);
}

#[test]
fn last_stage_runs_carry_their_fog_class_after_every_earlier_stage() {
    // Class = state * 4 + fog kind (bit 0: surface's last stage). Two
    // two-stage surfaces: the fog redraw of run 1 (last stages) must come
    // after run 0 (first stages), and the classes stay separate.
    let mut scratch = InlineDrawScratch::default();
    plan_inline_runs(&[0, 5, 0, 5], &[false, true, false, true], 8, &mut scratch);
    assert_eq!(scratch.runs, vec![(0, 2), (2, 2)]);
    assert_eq!(scratch.run_class, vec![0, 5]);
    // A last stage (class 1) never joins a first-stage run (class 0) of
    // the same state, so fog is only redrawn for finished surfaces.
    plan_inline_runs(&[0, 1, 0], &[false, false, false], 8, &mut scratch);
    assert_eq!(scratch.runs, vec![(0, 2), (2, 1)]);
    assert_eq!(scratch.run_class, vec![0, 1]);
}

#[test]
fn every_batch_is_drawn_exactly_once_in_a_valid_order() {
    // Pseudo-random layouts: each position appears once, and no stage is
    // drawn before the earlier stage of its surface.
    let mut seed = 0x2545_f491_u32;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        seed
    };
    for _ in 0..200 {
        let n = 1 + (next() % 40) as usize;
        let groups = 1 + (next() % 5) as usize;
        let group_of: Vec<u32> = (0..n).map(|_| next() % groups as u32).collect();
        let continues: Vec<bool> = (0..n)
            .map(|position| position > 0 && next() % 3 == 0)
            .collect();
        let mut scratch = InlineDrawScratch::default();
        plan_inline_runs(&group_of, &continues, groups, &mut scratch);
        let mut drawn_at = vec![usize::MAX; n];
        for (rank, &position) in scratch.order.iter().enumerate() {
            assert_eq!(drawn_at[position as usize], usize::MAX, "drawn twice");
            drawn_at[position as usize] = rank;
        }
        assert!(
            drawn_at.iter().all(|&rank| rank != usize::MAX),
            "a batch was dropped"
        );
        for position in 1..n {
            if continues[position] {
                assert!(
                    drawn_at[position - 1] < drawn_at[position],
                    "stage drawn before its predecessor"
                );
            }
        }
        let run_total: u32 = scratch.runs.iter().map(|&(_, count)| count).sum();
        assert_eq!(run_total as usize, n);
        assert_eq!(scratch.run_class.len(), scratch.runs.len());
        // Every run holds a single state.
        for &(first, count) in &scratch.runs {
            let states: std::collections::BTreeSet<u32> = scratch.order
                [first as usize..(first + count) as usize]
                .iter()
                .map(|&position| group_of[position as usize])
                .collect();
            assert_eq!(states.len(), 1);
        }
    }
}
