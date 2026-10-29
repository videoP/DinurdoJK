//! Timeline clips borrow recording-owned playback contexts. A second context
//! is retained only for recordings used by overlapping clips.
use super::StudioDemoClip;

pub(super) fn same_recording(a: &StudioDemoClip, b: &StudioDemoClip) -> bool {
    matches_recording(a, &b.name, &b.game)
}

pub(super) fn matches_recording(clip: &StudioDemoClip, name: &str, game: &str) -> bool {
    clip.name.eq_ignore_ascii_case(name) && clip.game.eq_ignore_ascii_case(game)
}

fn same_recording_fade(a: &StudioDemoClip, b: &StudioDemoClip) -> bool {
    use super::super::render_studio_timeline::StudioDemoTransition;
    same_recording(a, b) && a.layer == b.layer && a.start < b.start
        && b.start + 0.00001 < a.end() && b.transition_in != StudioDemoTransition::Cut
}

pub(super) fn requests(clips: &[StudioDemoClip], active: &[usize], time: f64, keep: bool) -> Vec<usize> {
    let mut result = active.to_vec();
    let mut chronological: Vec<_> = (0..clips.len()).collect();
    chronological.sort_by(|a, b| clips[*a].start.total_cmp(&clips[*b].start));
    // Warm the next recording or the second view of an upcoming same-demo fade.
    for &index in &chronological {
        if result.len() >= 2 { break; }
        let clip = &clips[index];
        if clip.start < time || result.contains(&index) { continue; }
        if result.iter().all(|&i| !same_recording(&clips[i], clip))
            || result.iter().any(|&i| same_recording_fade(&clips[i], clip)) {
            result.push(index);
            break;
        }
    }
    if !keep { return result; }
    // One context per unique recording, regardless of the number of cuts.
    for &index in &chronological {
        if result.iter().all(|&i| !same_recording(&clips[i], &clips[index])) { result.push(index); }
    }
    // At most two views of a recording. Keep the second warm across repeated
    // fades, but cuts and camera/Director shots alone never allocate another.
    for (position, &index) in chronological.iter().enumerate() {
        let clip = &clips[index];
        if result.iter().filter(|&&i| same_recording(&clips[i], clip)).count() >= 2 { continue; }
        if let Some(&other) = chronological[position + 1..].iter().find(|&&i|
            same_recording_fade(clip, &clips[i])) {
            result.push(if result.contains(&index) { other } else { index });
        }
    }
    result
}

/// `requests` under a retained-scene budget. The playing/next views are never
/// dropped; the optional retained extras are ranked upcoming-first (nearest
/// next clip first) and cut to `capacity` total contexts.
pub(super) fn requests_within(clips: &[StudioDemoClip], active: &[usize], time: f64, keep: bool, capacity: usize) -> Vec<usize> {
    let mut result = requests(clips, active, time, keep);
    let core = requests(clips, active, time, false).len().min(result.len());
    result[core..].sort_by(|&a, &b| {
        let (a, b) = (&clips[a], &clips[b]);
        (a.start < time).cmp(&(b.start < time)).then(a.start.total_cmp(&b.start))
    });
    result.truncate(core.max(capacity));
    result
}

pub(super) struct Candidate<'a> {
    pub id: u64,
    pub index: usize,
    pub name: &'a str,
    pub game: &'a str,
    pub sampled_ms: Option<i32>,
}

pub(super) fn assign(clips: &[StudioDemoClip], requests: &[usize], candidates: &[Candidate<'_>], time: f64) -> Vec<Option<u64>> {
    let mut result = vec![None; requests.len()];
    // Reserve exact existing bindings first so an earlier request cannot steal
    // the incoming side of a same-recording transition.
    for (slot, &index) in requests.iter().enumerate() {
        result[slot] = candidates.iter().find(|c| c.index == index
            && matches_recording(&clips[index], c.name, c.game)).map(|c| c.id);
    }
    for (slot, &index) in requests.iter().enumerate() {
        if result[slot].is_some() { continue; }
        let target = clips[index].source_time_ms(time.max(clips[index].start));
        result[slot] = candidates.iter().filter(|c| !result.contains(&Some(c.id))
            && matches_recording(&clips[index], c.name, c.game))
            .min_by_key(|c| (c.sampled_ms.map_or(i64::MAX, |last| (i64::from(last) - i64::from(target)).abs()), c.id))
            .map(|c| c.id);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn clip(name: &str, start: f64, end: f64) -> StudioDemoClip {
        StudioDemoClip { source_path: None, comparison: None, race_overlays: Vec::new(), pov_trail:Default::default(), hud_overrides:Default::default(), hide_other_players:false, name: name.into(), game: String::new(), start, duration_ms: 30_000,
            in_ms: 0, out_ms: ((end - start) * 1000.0) as i32, speed_keys: vec![],
            transition_in: Default::default(), audio: Default::default(), layer: 0 }
    }
    #[test]
    fn many_route_cuts_retain_two_recordings() {
        let clips: Vec<_> = (0..20).map(|i| clip(if i % 2 == 0 { "fast" } else { "detour" }, i as f64, i as f64 + 1.0)).collect();
        for i in 0..clips.len() {
            let plan = requests(&clips, &[i], i as f64 + 0.1, true);
            assert_eq!(plan.len(), 2);
            assert!(plan.contains(&i));
        }
    }
    #[test]
    fn same_recording_fades_need_two_contexts_not_one_per_clip() {
        let clips: Vec<_> = (0..12).map(|i| {
            let mut c = clip("a", i as f64, i as f64 + 1.5);
            c.transition_in = super::super::super::render_studio_timeline::StudioDemoTransition::Dissolve;
            c
        }).collect();
        assert_eq!(requests(&clips, &[0], 0.0, true).len(), 2);
        assert_eq!(requests(&clips, &[7, 8], 8.1, true), vec![7, 8]);
        assert_eq!(requests(&clips, &[11], 12.0, true).len(), 2);
    }
    #[test]
    fn layered_or_cut_overlaps_do_not_allocate_another_context() {
        let mut clips = vec![clip("a", 0.0, 3.0), clip("a", 1.0, 4.0)];
        assert_eq!(requests(&clips, &[1], 1.5, true).len(), 1);
        clips[1].transition_in = super::super::super::render_studio_timeline::StudioDemoTransition::Dissolve;
        clips[1].layer = 1;
        assert_eq!(requests(&clips, &[0], 0.5, true).len(), 1);
    }
    #[test]
    fn bindings_survive_reorder_and_preserve_both_sides_of_a_fade() {
        let clips = vec![clip("a", 0.0, 3.0), clip("a", 2.0, 5.0), clip("a", 5.0, 8.0)];
        let sources = [Candidate { id: 20, index: 2, name: "a", game: "", sampled_ms: Some(0) },
            Candidate { id: 21, index: 1, name: "a", game: "", sampled_ms: Some(100) }];
        assert_eq!(assign(&clips, &[0, 1], &sources, 2.1), vec![Some(20), Some(21)]);
        let reordered = vec![clip("a", 0.0, 2.0), clip("b", 2.0, 3.0)];
        assert_eq!(assign(&reordered, &[0, 1], &sources, 0.0), vec![Some(20), None]);
    }
    #[test]
    fn game_directories_are_distinct_and_low_memory_keeps_current_next_only() {
        let mut clips = vec![clip("a", 0.0, 1.0), clip("a", 1.0, 2.0), clip("c", 2.0, 3.0)];
        clips[1].game = "japro".into();
        assert_eq!(requests(&clips, &[0], 0.0, true).len(), 3);
        assert_eq!(requests(&clips, &[0], 0.0, false), vec![0, 1]);
        assert_eq!(requests(&[], &[], 0.0, true), Vec::<usize>::new());
    }
    #[test]
    fn scene_budget_keeps_current_next_and_nearest_upcoming_extras() {
        let clips = vec![clip("a", 0.0, 1.0), clip("b", 1.0, 2.0), clip("c", 2.0, 3.0),
            clip("d", 3.0, 4.0), clip("e", 4.0, 5.0)];
        assert_eq!(requests_within(&clips, &[2], 2.5, true, usize::MAX).len(), 5);
        // Current + next survive any budget; the nearest upcoming extra wins.
        assert_eq!(requests_within(&clips, &[2], 2.5, true, 0), vec![2, 3]);
        assert_eq!(requests_within(&clips, &[2], 2.5, true, 3), vec![2, 3, 4]);
        assert_eq!(requests_within(&clips, &[2], 2.5, true, 4), vec![2, 3, 4, 0]);
    }
}
