//! Studio Camera Director shots live in `jka_studio::render_studio_director`.
//! Route Comparison builds on race-ghost tracks and the client race camera.
pub(in crate::app) use jka_studio::render_studio_director::*;

/// Background Route Comparison analysis. `signature` is the editor state it
/// was started from; a result for a changed timeline is discarded.
pub struct StudioRouteJob {
    pub worker: std::thread::JoinHandle<Result<StudioRoutePlan, String>>,
    pub signature: String,
}

// ---------------------------------------------------------------------------
// Route Comparison: one shared-clock ghost race with local, explicitly labeled
// slowdown/pause/rewind/replay/return phases. Spatial alignment identifies local
// route differences; aligned section replays preserve each runner's true speed.
// ---------------------------------------------------------------------------

use super::render_studio_timeline::StudioDemoClip;

#[derive(Clone, Debug)]
pub struct StudioRouteOptions {
    /// None selects the fastest run; otherwise use this recording as the POV.
    pub main: Option<(String, String)>,
    pub moving_camera: bool,
    pub replay_trails: bool,
}

impl Default for StudioRouteOptions {
    fn default() -> Self {
        Self { main: None, moving_camera: true, replay_trails: true }
    }
}

/// Position samples are resampled to this spacing before matching.
const ROUTE_STEP_MS: i32 = 120;
/// A spatial difference must persist this long to count as a route split.
const ROUTE_MIN_SPLIT_MS: i32 = 450;
/// Lead-in before a split / tail after it, so a cut never lands on the fork.
const ROUTE_LEAD_MS: i32 = 600;
/// Find candidate forks once at the smallest supported tolerance. Changing
/// tolerance then merges routes inside stable, complete replay windows instead
/// of moving their gates or replacing one featured fork with another.
const ROUTE_MIN_TOLERANCE: f32 = 64.0;
/// Runs must start and finish near the fastest run's start / finish.
const ROUTE_ENDPOINT_RADIUS: f32 = 768.0;
/// Window membership is independent of the route grouping tolerance.
const ROUTE_GATE_RADIUS: f32 = 160.0;

/// One run, reduced to what route analysis needs (built on the worker).
#[derive(Clone, Debug)]
pub struct StudioRouteRun {
    pub name: String,
    pub game: String,
    pub map: String,
    /// Recording length on the Studio source clock (StudioDemoClip::duration_ms).
    pub duration_ms: i32,
    /// `duelTime` reset on the Studio source clock (may be < 0 if the
    /// recording began mid-race).
    pub race_source_start_ms: i32,
    /// (race elapsed ms, origin) for the selected race segment.
    pub samples: Vec<(i32, [f32; 3])>,
    /// The decoded track, kept for baking the chase camera.
    pub track: Option<std::sync::Arc<super::race_ghost::RaceGhostTrack>>,
}

impl StudioRouteRun {
    pub fn race_ms(&self) -> i32 { self.samples.last().map_or(0, |s| s.0) }
    /// Position at race time `t` (linear between snapshots).
    fn at(&self, t: i32) -> [f32; 3] {
        let i = self.samples.partition_point(|s| s.0 <= t);
        if i == 0 { return self.samples[0].1; }
        if i >= self.samples.len() { return self.samples[self.samples.len() - 1].1; }
        let (a, b) = (self.samples[i - 1], self.samples[i]);
        let u = ((t - a.0) as f32 / (b.0 - a.0).max(1) as f32).clamp(0.0, 1.0);
        std::array::from_fn(|k| a.1[k] + (b.1[k] - a.1[k]) * u)
    }
    /// Evenly resampled route (every ROUTE_STEP_MS of race time).
    fn resampled(&self) -> Vec<[f32; 3]> {
        (0..=self.race_ms() / ROUTE_STEP_MS).map(|i| self.at(i * ROUTE_STEP_MS)).collect()
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Distance from `p` to the segment a-b. Matching against the fastest run's
/// path (not its samples) keeps the error ~0 on an identical line at any
/// speed; sample spacing grows with speed (120 ms at 1000 ups = 120 units).
fn segment_dist(p: [f32; 3], a: [f32; 3], b: [f32; 3]) -> f32 {
    let ab: [f32; 3] = std::array::from_fn(|k| b[k] - a[k]);
    let len2 = ab.iter().map(|v| v * v).sum::<f32>();
    if len2 <= 1e-6 { return dist(p, a); }
    let u = ((0..3).map(|k| (p[k] - a[k]) * ab[k]).sum::<f32>() / len2).clamp(0.0, 1.0);
    dist(p, std::array::from_fn(|k| a[k] + ab[k] * u))
}

/// Global monotone path matching. Unlike a greedy forward band, this can
/// recover after a long shortcut or a loop instead of remaining stuck at a fork.
/// The bounded coarse grid uses at most 1 MiB of backtracking storage.
pub fn spatial_alignment(fast: &[[f32; 3]], alt: &[[f32; 3]]) -> Vec<(usize, f32)> {
    if fast.is_empty() || alt.is_empty() { return Vec::new(); }
    let indices = |len: usize| -> Vec<usize> {
        let n = len.min(1024);
        (0..n).map(|i| i * (len - 1) / (n - 1).max(1)).collect()
    };
    let (fi, ai) = (indices(fast.len()), indices(alt.len()));
    let n = fi.len();
    let distance = |i: usize, j: usize| {
        let end = fi.get(j + 1).copied().unwrap_or(fi[j]);
        segment_dist(alt[ai[i]], fast[fi[j]], fast[end])
    };
    let mut back = vec![0u8; n * ai.len()];
    let mut previous = vec![f32::INFINITY; n];
    for i in 0..ai.len() {
        let mut row = vec![f32::INFINITY; n];
        for j in 0..n {
            let candidates = [if i > 0 && j > 0 {previous[j-1]} else {f32::INFINITY},
                if i > 0 {previous[j]} else {f32::INFINITY},
                if j > 0 {row[j-1]} else {f32::INFINITY}];
            let direction = (0..3).min_by(|&a,&b| candidates[a].total_cmp(&candidates[b])).unwrap();
            row[j] = distance(i,j) + 1.0 + if i == 0 && j == 0 {0.0} else {candidates[direction]};
            back[i*n+j] = direction as u8;
        }
        previous = row;
    }
    let mut matches = vec![(0usize, f32::INFINITY); ai.len()];
    let (mut i, mut j) = (ai.len()-1, n-1);
    loop {
        let d = distance(i,j);
        if d < matches[i].1 {matches[i]=(j,d);}
        if i == 0 && j == 0 {break;}
        match back[i*n+j] {0=>{i=i.saturating_sub(1);j=j.saturating_sub(1);},1=>i=i.saturating_sub(1),_=>j=j.saturating_sub(1)}
    }
    let mut row = 0;
    let mut last = 0;
    alt.iter().enumerate().map(|(k,&p)| {
        while row+1 < ai.len() && ai[row+1] <= k {row+=1;}
        let end_row = if ai[row] == k {row} else {(row+1).min(ai.len()-1)};
        let start = last.max(fi[matches[row].0]);
        let end = fi.get(matches[end_row].0+1).copied().unwrap_or(fast.len()-1).max(start);
        let (index,d) = (start..=end).map(|s| (s,segment_dist(p,fast[s],fast[(s+1).min(fast.len()-1)])))
            .min_by(|a,b| a.1.total_cmp(&b.1)).unwrap();
        last=index;
        (index,d)
    }).collect()
}

/// Spatially matched progress, including fractional positions between the
/// analysis samples. Monotonicity keeps loops from jumping back to an earlier
/// crossing. Live gaps on different routes are course-aligned estimates;
/// finishing gaps use the exact recorded times.
pub fn course_progress(fast: &[[f32;3]], alt: &[[f32;3]], duration: i32, reference_duration: i32) -> Vec<[i32;2]> {
    let alignment=spatial_alignment(fast,alt);
    let mut result=vec![[0,0]];
    let mut last=0;
    let stride=(alt.len()/2048).max(1);
    for k in (1..alt.len()).step_by(stride) {
        let j=alignment[k].0;
        let a=glam::Vec3::from_array(fast[j]);
        let ab=glam::Vec3::from_array(fast[(j+1).min(fast.len()-1)])-a;
        let u=((glam::Vec3::from_array(alt[k])-a).dot(ab)/ab.length_squared().max(1e-6)).clamp(0.0,1.0);
        let p=(((j as f32+u)*ROUTE_STEP_MS as f32).round() as i32).clamp(last,reference_duration);
        let clock=k as i32*ROUTE_STEP_MS;
        if clock>=duration {break;}
        result.push([clock,p]);last=p;
    }
    result.push([duration,reference_duration]);result
}

/// A route split of one alternate: index ranges on both resampled routes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RouteSplit {
    pub fast: (usize, usize),
    pub alt: (usize, usize),
    /// Peak separation, map units.
    pub peak: f32,
}

/// Persistent stretches where the alternate is farther than `threshold`
/// from the fastest line.
pub fn route_splits(alignment: &[(usize, f32)], threshold: f32) -> Vec<RouteSplit> {
    let min_len = (ROUTE_MIN_SPLIT_MS / ROUTE_STEP_MS).max(1) as usize;
    let mut out = Vec::new();
    let mut start = None::<usize>;
    for j in 0..=alignment.len() {
        let off = alignment.get(j).is_some_and(|m| m.1 > threshold);
        match (off, start) {
            (true, None) => start = Some(j),
            (false, Some(s)) => {
                if j - s >= min_len {
                    let peak = alignment[s..j].iter().map(|m| m.1).fold(0.0, f32::max);
                    out.push(RouteSplit { fast: (alignment[s].0, alignment[j - 1].0), alt: (s, j - 1), peak });
                }
                start = None;
            }
            _ => {}
        }
    }
    out
}

/// A comparison event: one stretch of the fastest route where one or more
/// alternates split away. `alts` = (run index, alternate index range).
#[derive(Clone, Debug)]
struct RouteEvent {
    fast: (usize, usize),
    score: f32,
}

/// Match each fork against its original seed, never a successively growing
/// union. Otherwise overlapping differences from different runners can chain
/// unrelated forks into a replay of the entire course.
fn select_route_events(mut events: Vec<RouteEvent>) -> Vec<RouteEvent> {
    events.sort_by(|a,b|b.score.total_cmp(&a.score).then(a.fast.cmp(&b.fast)));
    let mut grouped=Vec::new();
    while !events.is_empty() {
        let seed=events.remove(0);let mut merged=seed.clone();
        events.retain(|event| {
            let overlap=(seed.fast.1.min(event.fast.1)+1).saturating_sub(seed.fast.0.max(event.fast.0));
            let a=seed.fast.1-seed.fast.0+1;let b=event.fast.1-event.fast.0+1;
            let same=overlap as f64/a.min(b) as f64>=0.75 && overlap as f64/a.max(b) as f64>=0.4;
            if same {merged.fast.0=merged.fast.0.min(event.fast.0);merged.fast.1=merged.fast.1.max(event.fast.1);}
            !same
        });
        grouped.push(merged);
    }
    let mut chosen:Vec<RouteEvent>=Vec::new();
    let mut anchors:Vec<(usize,usize)>=Vec::new();
    for event in grouped {
        let overlap=anchors.iter().enumerate().map(|(i,&(a,b))|
            (i,(b.min(event.fast.1)+1).saturating_sub(a.max(event.fast.0))))
            .max_by_key(|&(_,overlap)|overlap);
        if let Some((i,_))=overlap.filter(|&(_,n)|n as f64/(event.fast.1-event.fast.0+1) as f64>0.5) {
            // Include a related runner's earlier fork / later rejoin, while
            // keeping the original anchor fixed to prevent chained growth.
            chosen[i].fast.0=chosen[i].fast.0.min(event.fast.0);
            chosen[i].fast.1=chosen[i].fast.1.max(event.fast.1);
            continue;
        }
        anchors.push(event.fast);chosen.push(event);
    }
    chosen.sort_by_key(|e|e.fast.0);chosen
}

#[derive(Clone, Debug, Default)]
pub struct StudioRoutePlan {
    pub clips: Vec<StudioDemoClip>,
    /// Auto Director shots over the whole edit (ids assigned on commit).
    pub shots: Vec<StudioDirectorShot>,
    pub summary: String,
}

/// Largest separation sustained for 450 ms, in either direction. Sampling
/// cadence is local to the section, rather than ROUTE_STEP_MS.
fn section_route_difference(a:&[[f32;3]], b:&[[f32;3]], a_ms:i32, b_ms:i32) -> f32 {
    [(a,b,b_ms),(b,a,a_ms)].into_iter().map(|(target,subject,span)| {
        let matches=spatial_alignment(target,subject);
        let step=f64::from(span)/(subject.len()-1) as f64;
        let count=(f64::from(ROUTE_MIN_SPLIT_MS)/step).ceil().max(1.0) as usize;
        if count>matches.len() {return 0.0;}
        matches.windows(count).map(|w|w.iter().map(|m|m.1).fold(f32::INFINITY,f32::min))
            .fold(0.0,f32::max)
    }).fold(0.0,f32::max)
}

/// Complete-link hierarchy, cut at the requested tolerance. Its merge order
/// depends only on geometry, so lowering tolerance can only split groups; a
/// greedy first-fit pass can reshuffle them and even remove replay subjects.
fn unique_section_routes(runs:&[StudioRouteRun],entries:&[i32],exits:&[i32],reference:usize,threshold:f32,eligible:&[bool])
    ->Vec<super::render_studio_race::RaceRouteGroup> {
    let paths:Vec<Vec<[f32;3]>>=runs.iter().enumerate().map(|(i,r)| {
        let count=((exits[i]-entries[i])/80+1).clamp(12,512) as usize;
        (0..count).map(|k|r.at(entries[i]+((exits[i]-entries[i]) as i64*k as i64/(count-1) as i64) as i32)).collect()
    }).collect();
    let mut groups:Vec<_>=std::iter::once(reference).chain((0..runs.len()).filter(|&i|i!=reference && eligible[i]))
        .map(|index|super::render_studio_race::RaceRouteGroup{focus:index,members:vec![index]}).collect();
    let mut differences=vec![vec![0.0;runs.len()];runs.len()];
    for a in 0..groups.len() {
        for b in a+1..groups.len() {
            let (i,j)=(groups[a].focus,groups[b].focus);
            let d=section_route_difference(&paths[i],&paths[j],exits[i]-entries[i],exits[j]-entries[j]);
            differences[i][j]=d;differences[j][i]=d;
        }
    }
    loop {
        let mut closest=None::<(usize,usize,f32)>;
        for a in 0..groups.len() {
            for b in a+1..groups.len() {
                let d=groups[a].members.iter().flat_map(|&i|groups[b].members.iter().map(move |&j|(i,j)))
                    .map(|(i,j)|differences[i][j]).fold(0.0,f32::max);
                if closest.is_none_or(|(_,_,best)|d<best) {closest=Some((a,b,d));}
            }
        }
        let Some((a,b,_))=closest.filter(|&(_,_,d)|d<=threshold.max(ROUTE_MIN_TOLERANCE)) else {break};
        let other=groups.remove(b);groups[a].members.extend(other.members);groups[a].members.sort_unstable();
    }
    for group in &mut groups {
        group.focus=*group.members.iter().min_by_key(|&&i|(exits[i]-entries[i],i)).unwrap();
    }
    groups[1..].sort_by_key(|g|(exits[g.focus]-entries[g.focus],g.focus));
    groups
}

/// Build the edit. Pure: no I/O, deterministic for the same runs.
pub fn build_route_comparison_plan(runs: &[StudioRouteRun], threshold: f32, main: Option<usize>)
    -> Result<StudioRoutePlan, String> {
    if !(2..=32).contains(&runs.len()) {
        return Err(format!("Route comparison needs 2-32 different recordings (found {})", runs.len()));
    }
    let map = &runs[0].map;
    if let Some(other) = runs.iter().find(|r| !r.map.eq_ignore_ascii_case(map)) {
        return Err(format!("{} is on {}, {} is on {}: all runs must be the same map", runs[0].name, map, other.name, other.map));
    }
    if let Some(bad) = runs.iter().find(|r| r.samples.len() < 4 || r.race_ms() < 1000 || r.duration_ms <= 0
        || r.samples.iter().any(|s| s.1.iter().any(|v| !v.is_finite()))) {
        return Err(format!("{} has no usable race position samples", bad.name));
    }
    if main.is_some_and(|i| i >= runs.len()) { return Err("The main POV recording is no longer on the timeline".into()); }
    if !threshold.is_finite() {return Err("Route tolerance must be finite".into());}
    let fastest = main.unwrap_or_else(|| (0..runs.len()).min_by_key(|&i| runs[i].race_ms()).unwrap());
    let fast = &runs[fastest];
    let fast_route = fast.resampled();
    let (start, finish) = (fast_route[0], fast_route[fast_route.len() - 1]);
    let mut skipped = Vec::new();
    let mut events = Vec::<RouteEvent>::new();
    for (index, run) in runs.iter().enumerate() {
        if index == fastest { continue; }
        let route = run.resampled();
        if dist(route[0], start) > ROUTE_ENDPOINT_RADIUS || dist(route[route.len() - 1], finish) > ROUTE_ENDPOINT_RADIUS {
            skipped.push(format!("{} (different start/finish)", run.name));
            continue;
        }
        let alignment = spatial_alignment(&fast_route, &route);
        for split in route_splits(&alignment, ROUTE_MIN_TOLERANCE) {
            events.push(RouteEvent {fast:split.fast,score:split.peak});
        }
    }
    use super::render_studio_race::{RaceRunner, RaceSection, build_sequence};
    let chosen=select_route_events(events);
    let gate=ROUTE_GATE_RADIUS;
    let alignments:Vec<_>=runs.iter().map(|r|spatial_alignment(&fast_route,&r.resampled())).collect();
    let mut sections=Vec::new();
    let mut window_cursor=0;
    for event in chosen {
        let start_ms=(event.fast.0 as i32*ROUTE_STEP_MS-ROUTE_LEAD_MS).max(0);
        let end_ms=(event.fast.1 as i32*ROUTE_STEP_MS+ROUTE_LEAD_MS).min(fast.race_ms());
        // Reserve windows before tolerance filtering. Otherwise a newly
        // detected early fork can displace a later replay in build_sequence.
        if end_ms-start_ms<1000 || end_ms-500<window_cursor+500 || start_ms>=end_ms-500 {continue;}
        window_cursor=end_ms+500;
        let mut entries=Vec::new();let mut exits=Vec::new();
        for (i,run) in runs.iter().enumerate() {
            if i==fastest {entries.push(start_ms);exits.push(end_ms);continue;}
            let alignment=&alignments[i];
            let nearest=|ms:i32,from:usize| {
                let wanted=(ms/ROUTE_STEP_MS).max(0) as usize;
                (from..alignment.len()).min_by(|&a,&b|alignment[a].0.abs_diff(wanted).cmp(&alignment[b].0.abs_diff(wanted))
                    .then_with(||alignment[a].1.total_cmp(&alignment[b].1))).unwrap_or(from.min(alignment.len()-1))
            };
            let a=nearest(start_ms,0);let b=nearest(end_ms,a);
            // Refine the coarse path match against original snapshot positions.
            // Representative selection uses real section time, not rounded
            // 120 ms analysis samples or the runner's overall finishing time.
            let refine=|coarse:usize,gate:[f32;3],low:i32| {
                let center=coarse as i32*ROUTE_STEP_MS;
                run.samples.iter().filter(|s|s.0>=low&&(s.0-center).abs()<=2*ROUTE_STEP_MS)
                    .min_by(|a,b|dist(a.1,gate).total_cmp(&dist(b.1,gate))).map_or(center.max(low),|s|s.0)
            };
            let entry=refine(a,fast.at(start_ms),0).clamp(0,run.race_ms()-120);
            let exit=refine(b,fast.at(end_ms),entry+1).clamp(entry+1,run.race_ms());
            entries.push(entry);exits.push(exit);
        }
        // A detour crossing this window's exit stays a background companion;
        // feature it in its own complete fork-to-rejoin window, not a clipped
        // fragment of a longer branch.
        let eligible:Vec<_>=runs.iter().enumerate().map(|(i,r)| i==fastest ||
            (start_ms==0 || dist(r.at(entries[i]),fast.at(start_ms))<=gate) &&
            ((end_ms==fast.race_ms() && r.race_ms()-exits[i]<=ROUTE_LEAD_MS) || dist(r.at(exits[i]),fast.at(end_ms))<=gate)
        ).collect();
        for (i,&complete) in eligible.iter().enumerate() {
            if !complete {exits[i]=runs[i].race_ms();}
        }
        let routes=unique_section_routes(runs,&entries,&exits,fastest,threshold,&eligible);
        if routes.len()>1 {sections.push(RaceSection{start_ms,end_ms,entries,exits,routes});}
    }
    let runners = runs.iter().enumerate().map(|(i,r)| RaceRunner {
        name:r.name.clone(),game:r.game.clone(),duration_ms:r.duration_ms,race_ms:r.race_ms(),
        color:super::render_studio_race::runner_color(i,runs.len(),fastest),
        progress:if i==fastest {vec![[0,0],[r.race_ms(),r.race_ms()]]}
            else {course_progress(&fast_route,&r.resampled(),r.race_ms(),fast.race_ms())},
    }).collect();
    let comparison = build_sequence(runners, fastest, fast.race_source_start_ms, fast.duration_ms, sections, true, true);
    let highlights=comparison.phases.iter().filter(|p|p.kind==super::render_studio_race::RacePhaseKind::Replay).count();
    let section_count=comparison.phases.iter().map(|p|p.section).max().unwrap_or(0);
    let length = comparison.duration();
    let clip = StudioDemoClip { demo_protocol:26, source_path: None,
        comparison:Some(Box::new(comparison)),race_overlays:Vec::new(),pov_trail:Default::default(),
        hud_overrides:Default::default(),hide_other_players:true,name:fast.name.clone(),game:fast.game.clone(),
        start:0.0,duration_ms:fast.duration_ms,in_ms:0,out_ms:fast.duration_ms,speed_keys:vec![],
        transition_in:Default::default(),audio:Default::default(),layer:0,
    };
    if !clip.valid() {return Err("The ghost comparison sequence failed clock validation".into());}
    Ok(StudioRoutePlan {clips:vec![clip],shots:vec![],
        summary:format!("Ghost race on {map}: {} runners together | reference {} ({:.2}s) | tolerance {threshold:.0} u | {section_count} forks | {highlights} unique-route replays | {length:.1}s{}",
            runs.len(),fast.name,fast.race_ms() as f64/1000.0,
            if skipped.is_empty(){String::new()}else{format!(" | endpoint differences: {}",skipped.join(", "))})})
}

/// Every phase uses the same ghost assignments, including the reference actor.
pub fn inherit_route_presentation(plan: &mut StudioRoutePlan, prior: &[StudioDemoClip], options: &StudioRouteOptions) {
    let Some(clip) = plan.clips.first_mut() else {return};
    let original = prior.iter().find(|c| c.name == clip.name && c.game == clip.game);
    if let Some(source) = original {
        clip.source_path=source.source_path.clone();
        clip.hud_overrides=source.hud_overrides.clone(); clip.audio=source.audio.clone();
        clip.audio.fade_in=clip.audio.fade_in.min(clip.duration());
        clip.audio.fade_out=clip.audio.fade_out.min(clip.duration());
    }
    let Some(race) = clip.comparison.as_mut() else {return};
    race.orbits=options.moving_camera;
    race.trails=options.replay_trails;
    for (i,runner) in race.runners.iter_mut().enumerate() {
        let existing = original.and_then(|c| c.race_overlays.iter().find(|o| o.name==runner.name && o.game==runner.game));
        let source = prior.iter().find(|c| c.name==runner.name && c.game==runner.game);
        let color = existing.and_then(|o|o.trail_color).or_else(||source.and_then(|c|c.pov_trail.trail_color));
        if let Some(color) = color {runner.color=color;}
        let source_path=source.and_then(|c|c.source_path.clone())
            .or_else(||existing.and_then(|o|o.source_path.clone()));
        clip.race_overlays.push(super::render_studio_timeline::StudioRaceOverlay {
            source_path,name:runner.name.clone(),game:runner.game.clone(),enabled:true,
            alpha:if i==race.reference {1.0}else{existing.map_or(0.65,|o|o.alpha)},
            strafe_trail:options.replay_trails,trail_ticks:existing.map_or(false,|o|o.trail_ticks)
                || source.is_some_and(|c|c.pov_trail.trail_ticks),
            trail_tick_labels:existing.map_or_else(Default::default,|o|o.trail_tick_labels),trail_color:Some(runner.color),
        });
    }
    // The reference is rendered exactly once, through the indexed ghost path.
    clip.pov_trail.enabled=false;
    clip.hide_other_players=true;
}

/// The camera is evaluated from indexed ghost poses at runtime. These authored
/// phase spans make the sequence readable in the timeline and project files.
pub fn direct_route_plan(plan: &mut StudioRoutePlan, fov: f32, seed: u32) {
    plan.shots.clear();
    let Some(clip)=plan.clips.first() else{return};
    let Some(race)=clip.comparison.as_ref() else{return};
    for phase in &race.phases {
        let mut shot=StudioDirectorShot::new(0,StudioDirectorPreset::Chase,-1,clip.start+phase.start,phase.duration);
        shot.fov=fov;shot.seed=seed;shot.auto=true;
        shot.comparison_label=Some(format!("{}{}",race.phase_label(phase),if phase.section>0{format!(" / section {}",phase.section)}else{String::new()}));
        plan.shots.push(shot);
    }
}

/// Worker: solve the comparison's chase camera once, ahead of time, against
/// map collision (`render_studio_race::bake_camera`). Without collision or
/// tracks the comparison keeps the per-frame solve.
pub fn bake_route_camera(plan: &mut StudioRoutePlan, runs: &[StudioRouteRun],
    world: Option<jka_movement::CollisionWorld>, viewport: [u32; 2]) {
    let Some(race) = plan.clips.first_mut().and_then(|c| c.comparison.as_mut()) else {return};
    let tracks: Vec<_> = race.runners.iter().map(|runner| runs.iter()
        .find(|r| r.name == runner.name && r.game == runner.game).and_then(|r| r.track.as_deref())).collect();
    let Some(mut world) = world.filter(|_| tracks.iter().all(Option::is_some)) else {
        plan.summary.push_str(" | map collision unavailable: live chase camera");
        return;
    };
    let keys = super::render_studio_race::bake_camera(race, &tracks, &mut world, viewport);
    race.camera = keys;
    race.camera_version = super::render_studio_race::CAMERA_VERSION;
}

/// Worker: decode each recording's last `duelTime` race segment.
pub fn load_route_runs(base: std::path::PathBuf, project_dir: std::path::PathBuf,
    media: std::collections::HashMap<(String,String),String>, inputs: Vec<(String, String, i32)>)
    -> Result<Vec<StudioRouteRun>, String> {
    inputs.into_iter().map(|(name, game, duration_ms)| {
        let reference=media.get(&(name.clone(),game.clone()))
            .ok_or_else(||format!("No project-managed recording for {name}"))?;
        let path=super::render_studio_media::media_path(&project_dir,reference)?;
        let meta=std::fs::metadata(&path).map_err(|e|e.to_string())?;
        if meta.len()==0 || meta.len()>512*1024*1024 {return Err("Invalid route demo size".into());}
        let bytes=std::fs::read(path).map_err(|e|e.to_string())?;
        let track = super::race_ghost::parse_track(name.clone(), bytes)
            .map_err(|e| format!("{name}: {e}"))?;
        let samples = track.frames.iter().filter_map(|frame| {
            super::playerstate_vec3(&frame.player_state, "origin").map(|o| (frame.elapsed_ms, o))
        }).collect();
        Ok(StudioRouteRun { name, game, map: track.map_name.clone(), duration_ms,
            race_source_start_ms: track.race_source_start_ms, samples, track: Some(std::sync::Arc::new(track)) })
    }).collect()
}

#[cfg(test)]
pub fn route_test_inputs(base:&std::path::Path) -> (Vec<(String,String,i32)>,Vec<StudioDemoClip>) {
    if let Some(path)=std::env::var_os("DJK_STUDIO_TEST_PROJECT") {
        let json:serde_json::Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let prior:Vec<StudioDemoClip>=serde_json::from_value(json["demos"].clone()).unwrap();
        let mut inputs=Vec::new();
        for clip in &prior {
            let entries=if let Some(race)=&clip.comparison {race.runners.iter().map(|r|(r.name.clone(),r.game.clone(),r.duration_ms)).collect::<Vec<_>>()}
                else {vec![(clip.name.clone(),clip.game.clone(),clip.duration_ms)]};
            for entry in entries {if !inputs.iter().any(|e:&(String,String,i32)|e.0==entry.0&&e.1==entry.1) {inputs.push(entry);}}
        }
        return (inputs,prior);
    }
    let folder=base.parent().unwrap().join("japro/demos");
    let inputs=["acrobat","aivi","chouan2","katy","lemonlime","shakur","thndr","tobi","vrael"].iter().map(|person| {
        let name=format!("{person}-ventara(echidna-mountain)-{}",if *person=="tobi"{"sp"}else{"jka"});
        let path=super::render_studio_auto_director::parse_recorder_path(std::fs::read(folder.join(format!("{name}.dm_26"))).unwrap()).unwrap();
        (name,"japro".into(),path.samples.last().unwrap().ms)
    }).collect();(inputs,Vec::new())
}

#[cfg(test)]
mod route_tests {
    use super::*;

    #[test]
    fn overlapping_forks_do_not_chain_into_a_full_course_replay() {
        let selected=select_route_events(vec![RouteEvent{fast:(0,100),score:2.0},RouteEvent{fast:(80,180),score:1.0},RouteEvent{fast:(160,260),score:3.0}]);
        assert_eq!(selected.len(),3);
        assert!(selected.iter().all(|e|e.fast.1-e.fast.0==100));
        let full=select_route_events(vec![RouteEvent{fast:(0,1000),score:10.0},RouteEvent{fast:(20,100),score:2.0}]);
        assert_eq!(full.len(),1);assert_eq!(full[0].fast,(0,1000),"preserve a genuinely long detour");
        let related=select_route_events(vec![RouteEvent{fast:(0,100),score:10.0},RouteEvent{fast:(120,220),score:9.0},RouteEvent{fast:(110,180),score:5.0}]);
        assert_eq!(related.len(),2);assert_eq!(related[1].fast,(110,220),"include the alternate's real fork before the featured section");
    }

    fn run(name: &str, path: &[(f32, f32)], speed: f32) -> StudioRouteRun {
        // Constant-speed walk through the waypoints (units per ms = speed).
        let mut samples = Vec::new();
        let mut t = 0.0f32;
        for w in path.windows(2) {
            let (a, b) = (w[0], w[1]);
            let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
            let steps = (len / (speed * 40.0)).ceil().max(1.0) as i32;
            for s in 0..steps {
                let u = s as f32 / steps as f32;
                samples.push((t as i32, [a.0 + (b.0 - a.0) * u, a.1 + (b.1 - a.1) * u, 0.0]));
                t += len / steps as f32 / speed;
            }
        }
        let last = path[path.len() - 1];
        samples.push((t as i32, [last.0, last.1, 0.0]));
        StudioRouteRun { name: name.into(), game: String::new(), map: "racearena".into(),
            duration_ms: t as i32 + 5000, race_source_start_ms: 2000, samples, track: None }
    }


    use super::super::render_studio_race::RacePhaseKind as K;

    fn comparison_runs() -> Vec<StudioRouteRun> {
        vec![run("fast", &[(0.0,0.0),(8000.0,0.0)],1.0),
            run("detour", &[(0.0,0.0),(2000.0,0.0),(2500.0,900.0),(3500.0,900.0),(4000.0,0.0),(8000.0,0.0)],1.0)]
    }

    #[test]
    fn tolerance_merges_routes_without_shortening_the_more_detailed_edit() {
        let mut runs=vec![run("reference",&[(0.0,0.0),(10000.0,0.0)],1.0)];
        for height in [90.0,240.0,700.0,1800.0] {
            runs.push(run(&format!("detour_{height}"),&[(0.0,0.0),(2000.0,0.0),(2500.0,height),
                (5500.0,height),(6000.0,0.0),(10000.0,0.0)],1.0));
        }
        let entries=vec![0;runs.len()];let exits:Vec<_>=runs.iter().map(|r|r.race_ms()).collect();
        let mut previous=None::<(f64,Vec<super::super::render_studio_race::RaceRouteGroup>)>;
        let mut durations=Vec::new();
        for tolerance in [64.0,100.0,160.0,300.0,600.0,1024.0,2048.0,4096.0] {
            let plan=build_route_comparison_plan(&runs,tolerance,None).unwrap();
            let race=plan.clips[0].comparison.as_ref().unwrap();
            assert!(race.valid(plan.clips[0].duration_ms));
            let groups=unique_section_routes(&runs,&entries,&exits,0,tolerance,&vec![true;runs.len()]);
            if let Some((duration,prior))=&previous {
                assert!(race.duration()<=duration+1e-6,"raising tolerance to {tolerance} lengthened the edit");
                assert!(prior.iter().all(|a|groups.iter().any(|b|a.members.iter().all(|i|b.members.contains(i)))),
                    "raising tolerance must merge groups, never reshuffle them");
            }
            durations.push(race.duration());previous=Some((race.duration(),groups));
        }
        assert!(durations.windows(2).filter(|w|w[0]>w[1]+1e-6).count()>=4,"the control must change actual duration at several tolerances: {durations:?}");
        assert_eq!(*durations.last().unwrap(),runs.iter().map(|r|r.race_ms()).max().unwrap() as f64/1000.0);
        let a=build_route_comparison_plan(&runs,160.0,None).unwrap();
        let b=build_route_comparison_plan(&runs,160.0,None).unwrap();
        assert_eq!(serde_json::to_value(&a.clips).unwrap(),serde_json::to_value(&b.clips).unwrap());
    }

    #[test]
    fn independent_forks_are_not_capped_at_three() {
        let events=(0..6).map(|i|RouteEvent{fast:(i*100,i*100+40),score:1000.0-i as f32}).collect();
        assert_eq!(select_route_events(events).len(),6);
    }

    #[test]
    #[ignore = "Requires saved route samples; set DJK_STUDIO_ROUTE_SAMPLES to studio-route-analysis.json"]
    fn saved_route_tolerance_changes_finished_duration() {
        let path=std::env::var_os("DJK_STUDIO_ROUTE_SAMPLES").expect("DJK_STUDIO_ROUTE_SAMPLES");
        let data:serde_json::Value=serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let comparison:super::super::render_studio_race::RaceComparison=serde_json::from_value(data["comparison"].clone()).unwrap();
        let runs:Vec<_>=comparison.runners.iter().zip(data["paths"].as_array().unwrap()).map(|(r,p)|StudioRouteRun {
            name:r.name.clone(),game:r.game.clone(),map:"saved course".into(),duration_ms:r.duration_ms,
            race_source_start_ms:comparison.source_start_ms,samples:serde_json::from_value(p["samples"].clone()).unwrap(),track:None,
        }).collect();
        let mut previous=f64::INFINITY;let mut lengths=Vec::new();
        for tolerance in [64.0,100.0,160.0,300.0,600.0,1024.0,1600.0,2400.0,4096.0] {
            let plan=build_route_comparison_plan(&runs,tolerance,Some(comparison.reference)).unwrap();
            let race=plan.clips[0].comparison.as_ref().unwrap();
            let replays=race.phases.iter().filter(|p|p.kind==K::Replay).count();
            eprintln!("tolerance={tolerance:.0} replays={replays} duration={:.3}s",race.duration());
            assert!(race.valid(plan.clips[0].duration_ms));
            assert!(race.duration()<=previous+1e-6,"raising tolerance to {tolerance} lengthened the edit");
            previous=race.duration();lengths.push(previous);
        }
        assert!(lengths.windows(2).filter(|w|w[0]>w[1]+1e-6).count()>=3,"expected several distinct finished lengths: {lengths:?}");
    }

    #[test]
    fn alignment_ignores_speed_on_the_same_line() {
        let line=[(0.0,0.0),(4000.0,0.0)];
        let alignment=spatial_alignment(&run("fast",&line,1.0).resampled(),&run("slow",&line,0.5).resampled());
        assert!(alignment.iter().all(|m|m.1<40.0));
        assert!(alignment.windows(2).all(|w|w[0].0<=w[1].0));
        assert!(route_splits(&alignment,160.0).is_empty());
    }

    #[test]
    fn ghost_race_is_one_clip_and_all_runners_survive_every_phase() {
        let runs=comparison_runs();
        let mut plan=build_route_comparison_plan(&runs,160.0,None).unwrap();
        inherit_route_presentation(&mut plan,&[],&Default::default());
        direct_route_plan(&mut plan,90.0,7);
        assert_eq!(plan.clips.len(),1);
        let clip=&plan.clips[0];let race=clip.comparison.as_ref().unwrap();
        assert_eq!(clip.name,"fast");assert_eq!(clip.race_overlays.len(),2);
        assert!(clip.race_overlays.iter().all(|o|o.enabled&&o.strafe_trail));
        assert!(race.phases.iter().any(|p|p.kind==K::Replay));
        assert!(clip.valid());assert!(race.valid(clip.duration_ms));
        assert_eq!(race.runners[race.reference].name,"fast");
        assert_eq!(race.phases.iter().filter(|p|p.kind==K::Replay).count(),1);
        assert!(race.phases.iter().filter(|p|p.kind==K::Replay).all(|p|!p.replay.as_ref().unwrap().members.contains(&race.reference)),"the main chase already showed the reference line");
        assert!(race.phases.iter().filter(|p|p.kind==K::Rewind).all(|p|p.duration<=1.5),"rewinds must be brief");
        assert!(plan.shots.iter().all(|s|s.valid()&&s.comparison_label.is_some()));
        for p in &race.phases {
            let begin=race.sample(p.start);let end=race.sample(p.start+p.duration-0.00001);
            assert_eq!(begin.clocks,p.from);assert_eq!(end.clocks,p.to);
            assert_eq!(begin.world_ms,p.world_from);assert_eq!(end.world_ms,p.world_to);
            if p.kind==K::Pause {assert_eq!(begin.clocks,end.clocks);}
            if p.kind==K::Rewind {assert!(begin.clocks.iter().zip(&end.clocks).all(|(a,b)|a>=b));assert!(begin.clocks!=end.clocks);}
            if !matches!(p.kind,K::Race|K::Slowdown|K::Resume) {assert_eq!(p.world_from,p.world_to);}
            if p.kind==K::Replay {
                let a=race.sample(p.start+0.6);let b=race.sample(p.start+1.6);
                for i in 0..race.runners.len() {
                    if b.clocks[i]<p.to[i] {assert_eq!(b.clocks[i]-a.clocks[i],1000,"preserve real speed");}
                }
            }
        }
        let mut previous=-1;
        for i in 0..=(race.duration()*100.0) as usize {
            let sample=race.sample(i as f64/100.0);
            assert!(sample.world_ms>=previous,"native world never rewinds");previous=sample.world_ms;
        }
        let text=serde_json::to_string(clip).unwrap();
        let loaded:StudioDemoClip=serde_json::from_str(&text).unwrap();
        assert!(loaded.valid());
        let restored=loaded.comparison.as_ref().unwrap();
        assert_eq!(restored.runners,race.runners);
        for i in 0..(race.duration()*60.0) as usize {
            let time=i as f64/60.0;
            assert_eq!(restored.sample(time).clocks,race.sample(time).clocks);
            assert_eq!(restored.sample(time).world_ms,race.sample(time).world_ms);
        }
    }

    #[test]
    fn identical_lines_still_generate_simultaneous_ghosts_without_replays() {
        let runs=[run("fast",&[(0.0,0.0),(8000.0,0.0)],1.0),run("slow",&[(0.0,0.0),(8000.0,0.0)],0.5)];
        let plan=build_route_comparison_plan(&runs,160.0,None).unwrap();
        let clip=&plan.clips[0];let race=clip.comparison.as_ref().unwrap();
        assert_eq!(race.phases.len(),1);assert_eq!(race.phases[0].kind,K::Race);
        // Slower runners keep going after the reference finishes. The native
        // map clock follows the reference at 1x, then holds its finish.
        assert_eq!(race.sample(4.0).world_ms,6000);
        assert_eq!(race.sample(12.0).world_ms,10000);
        assert_eq!(race.duration(),16.0);
    }

    #[test]
    fn reference_can_be_selected_and_limits_are_two_to_thirty_two() {
        let mut runs=comparison_runs();
        let plan=build_route_comparison_plan(&runs,160.0,Some(1)).unwrap();
        assert_eq!(plan.clips[0].name,"detour");
        assert!(build_route_comparison_plan(&runs[..1],160.0,None).is_err());
        assert!(build_route_comparison_plan(&runs,160.0,Some(2)).is_err());
        for i in 2..32 {let mut r=runs[0].clone();r.name=format!("run{i}");runs.push(r);}
        assert!(build_route_comparison_plan(&runs,160.0,None).is_ok());
        runs.push(runs[0].clone());assert!(build_route_comparison_plan(&runs,160.0,None).is_err());
        runs.truncate(2);runs[1].map="different".into();
        assert!(build_route_comparison_plan(&runs,160.0,None).is_err());
    }

    #[test]
    fn comparison_preserves_hud_and_matching_ghost_colors_without_duplicate_reference() {
        let mut plan=build_route_comparison_plan(&comparison_runs(),160.0,None).unwrap();
        let mut original=plan.clips[0].clone();
        original.hud_overrides.insert("strafe_helper".into(),true);
        original.pov_trail.enabled=true;original.pov_trail.trail_ticks=true;
        original.pov_trail.trail_color=Some([10,20,30]);
        inherit_route_presentation(&mut plan,&[original.clone()],&Default::default());
        let clip=&plan.clips[0];let race=clip.comparison.as_ref().unwrap();
        assert_eq!(clip.hud_overrides,original.hud_overrides);assert!(!clip.pov_trail.enabled);
        assert_eq!(clip.race_overlays.iter().filter(|o|o.name=="fast").count(),1);
        assert_eq!(clip.race_overlays[0].trail_color,Some([10,20,30]));
        assert_eq!(race.runners[0].color,[10,20,30]);assert!(clip.race_overlays[0].trail_ticks);
        for p in &race.phases {
            let expected=if matches!(p.kind,K::Race|K::Replay|K::Slowdown|K::Resume) {1.0}else{0.0};
            assert_eq!(clip.audio_gain_at(p.start+p.duration*0.5),expected,"{:?}",p.kind);
        }
    }

    #[test]
    fn comparison_audio_resumes_after_rewinds_for_each_focused_runner() {
        let runs=[run("reference",&[(0.0,0.0),(8000.0,0.0)],1.0),
            run("upper",&[(0.0,0.0),(2000.0,0.0),(2500.0,4000.0),(7000.0,4000.0),(8000.0,0.0)],1.0),
            run("lower",&[(0.0,0.0),(2000.0,0.0),(2500.0,-4000.0),(7000.0,-4000.0),(8000.0,0.0)],1.0)];
        let mut plan=build_route_comparison_plan(&runs,160.0,Some(0)).unwrap();
        let clip=&mut plan.clips[0];clip.start=10.0;
        let race=clip.comparison.as_ref().unwrap();
        let replays:Vec<_>=race.phases.iter().filter(|p|p.kind==K::Replay).collect();
        assert_eq!(replays.len(),2,"exercise a second POV switch after another rewind");
        let mut held_reference=false;
        for replay in replays {
            let at=clip.start+replay.start+replay.duration*0.5;
            assert_ne!(race.focus(replay),race.reference);
            assert_eq!(clip.audio_gain_at(at),1.0,"the switched player's replay must be audible");
            assert_eq!(clip.audio_gain_within(at,clip.duration()),1.0);
            for step in 1..(replay.duration*10.0) as usize {
                let local=replay.start+step as f64/10.0;let sample=race.sample(local);
                let focus=race.focus(sample.phase);
                if sample.clocks[race.reference]>=race.runners[race.reference].race_ms
                    && sample.clocks[focus]<race.runners[focus].race_ms {
                    held_reference=true;
                    assert_eq!(clip.audio_gain_at(clip.start+local),1.0,"the reference finishing must not mute another player's replay");
                }
            }
        }
        assert!(held_reference,"exercise replay audio while the reference has finished");
        for phase in &race.phases {
            if matches!(phase.kind,K::Pause|K::Rewind|K::Align|K::Return) {
                assert_eq!(clip.audio_gain_at(clip.start+phase.start+phase.duration*0.5),0.0);
            }
        }
        let at=clip.start+race.phases.iter().find(|p|p.kind==K::Replay).unwrap().start+1.0;
        clip.audio.mute=true;assert_eq!(clip.audio_gain_at(at),0.0,"explicit clip mute still applies to replays");
    }

    #[test]
    fn unique_route_uses_fastest_section_not_overall_time() {
        let mut runs=comparison_runs();
        let mut quicker=runs[1].clone();quicker.name="late_start_fast_section".into();
        for sample in &mut quicker.samples {
            sample.0=if sample.0<500 {sample.0*8}else{4000+((sample.0-500) as f32*0.65).round() as i32};
        }
        quicker.duration_ms=quicker.race_ms()+5000;
        assert!(quicker.race_ms()>runs[1].race_ms());runs.push(quicker);
        let plan=build_route_comparison_plan(&runs,160.0,Some(0)).unwrap();
        let race=plan.clips[0].comparison.as_ref().unwrap();
        let replays:Vec<_>=race.phases.iter().filter(|p|p.kind==K::Replay).collect();
        assert_eq!(replays.len(),1,"replay the distinct detour once without repeating the reference line");
        let route=replays.iter().find_map(|p|p.replay.as_ref().filter(|r|r.members.contains(&1))).unwrap();
        assert_eq!(route.focus,2);assert_eq!(route.members,vec![1,2]);
        assert!(race.valid(plan.clips[0].duration_ms));
        for replay in replays {
            let index=race.phases.iter().position(|p|std::ptr::eq(p,replay)).unwrap();
            assert_eq!(race.phases[index-1].kind,K::Align);
            assert_eq!(race.phases[index-2].kind,K::Rewind,"every route has a visible rewind");
        }
    }

    #[test]
    fn long_unique_detour_runs_all_the_way_to_its_rejoin() {
        let fast=run("fast",&[(0.0,0.0),(8000.0,0.0)],1.0);
        let detour=run("long_detour",&[(0.0,0.0),(2000.0,0.0),(2500.0,4000.0),(3500.0,4000.0),(4000.0,0.0),(8000.0,0.0)],1.0);
        let runs=[fast,detour];let plan=build_route_comparison_plan(&runs,160.0,None).unwrap();
        let race=plan.clips[0].comparison.as_ref().unwrap();
        let phase=race.phases.iter().find(|p|p.kind==K::Replay&&p.replay.as_ref().is_some_and(|r|r.focus==1)).unwrap();
        let route=phase.replay.as_ref().unwrap();
        assert!(route.exit_ms-route.entry_ms>6500,"full detour must exceed the old cap");
        assert_eq!(phase.to[1],route.exit_ms);
        assert!(dist(runs[1].at(route.exit_ms),runs[0].at(phase.to[0]))<240.0,"both reach the rejoin");
    }

    #[test]
    #[ignore = "Requires local Ventara recordings and installed map assets; set DJK_STUDIO_TEST_BASE"]
    fn actual_ventara_ghost_race_keeps_every_runner_and_reference_in_frame() {
        use super::super::{race_ghost,render_studio_race as race,render_studio_demo::StudioRaceRuntime,render_studio_auto_director as auto};
        let base=std::path::PathBuf::from(std::env::var_os("DJK_STUDIO_TEST_BASE").expect("DJK_STUDIO_TEST_BASE"));
        jka_assets::pk3::configure_filesystem_roots(&base,jka_client_services::filesystem_paths::load(&base));
        let (inputs,prior)=route_test_inputs(&base);
        let mut runtime=Vec::new();
        for (i,(name,game,_)) in inputs.iter().enumerate() {
            let bytes=std::fs::read(base.parent().unwrap().join(game).join("demos").join(format!("{name}.dm_26"))).unwrap();
            let track=race_ghost::parse_track(name.clone(),bytes).unwrap();
            let mut ghost = race_ghost::RaceGhost::from_track(track,&[],60000+i as u16).unwrap();
            assert_eq!(ghost.info.saber_name,"none");
            ghost.enable_studio_presentation(&[]);
            assert_ne!(ghost.info.saber_name,"none","Studio must restore the recorded saber for {name}");
            assert!(ghost.studio_audio_events(0,true).is_empty());
            let mut sounds=0;
            for clock in (20..ghost.track.duration_ms()).step_by(20) {
                sounds+=ghost.studio_audio_events(clock,true).len();
                assert!(ghost.studio_audio_events(clock,true).is_empty(),"repeated preview frame must not repeat sound");
            }
            assert!(sounds>0,"recorded audio events for {name}");
            assert!(ghost.studio_audio_events(0,true).is_empty(),"rewind stays silent");
            assert!(ghost.studio_audio_events(10000,true).is_empty(),"seek must not burst old sounds");
            assert!(ghost.studio_audio_events(10020,false).is_empty(),"unfocused runner stays silent");
            ghost.studio_audio_events(0,false);
            runtime.push(StudioRaceRuntime{key:(name.clone(),game.clone()),ghost:Some(ghost),worker:None,pending_track:None,error:None});
        }
        let runs=load_route_runs(base.clone(),jka_assets::pk3::dinurdojk_directory(&base).join("render-studio/Test"),Default::default(),inputs).unwrap();
        let mut plan=build_route_comparison_plan(&runs,160.0,None).unwrap();
        inherit_route_presentation(&mut plan,&prior,&Default::default());direct_route_plan(&mut plan,90.0,7);
        eprintln!("{}",plan.summary);
        let clip=&plan.clips[0];let comparison=clip.comparison.as_ref().unwrap();
        assert_eq!(plan.clips.len(),1);assert_eq!(clip.race_overlays.len(),9);assert!(clip.valid());
        assert_eq!(comparison.runners[comparison.reference].name,"thndr-ventara(echidna-mountain)-jka");
        let focuses:Vec<_>=comparison.phases.iter().filter(|p|p.kind==K::Replay).map(|p|p.replay.as_ref().unwrap().focus).collect();
        eprintln!("Replay subjects: {:?}",focuses.iter().map(|&i|&runs[i].name).collect::<Vec<_>>());
        assert!(focuses.iter().any(|&i|i!=comparison.reference),"must show alternate routes");
        std::fs::create_dir_all("artifacts").unwrap();
        let paths:Vec<_>=runs.iter().map(|r|serde_json::json!({"name":r.name,"samples":r.samples,
            "splits":route_splits(&spatial_alignment(&runs[comparison.reference].resampled(),&r.resampled()),160.0).iter()
                .map(|s|serde_json::json!({"fast":s.fast,"alt":s.alt,"peak":s.peak})).collect::<Vec<_>>() })).collect();
        std::fs::write("artifacts/studio-route-analysis.json",serde_json::to_vec(&serde_json::json!({"paths":paths,"comparison":comparison})).unwrap()).unwrap();
        let mut collision=auto::load_collision(&base,&Default::default(),"japro",&runs[comparison.reference].map,None).expect("Ventara collision");
        let mut comparison=comparison.clone();
        let started=std::time::Instant::now();
        let keys={let tracks=race::runner_tracks(&comparison,&runtime);race::bake_camera(&comparison,&tracks,&mut collision,[1280,720])};
        eprintln!("Baked {} camera keys in {:?}",keys.len(),started.elapsed());
        assert!(!keys.is_empty());comparison.camera=keys;
        let comparison=&comparison;
        let mut previous=None::<glam::Vec3>;let mut largest=0.0f32;
        for i in 0..(comparison.duration()*20.0) as usize {
            let time=i as f64/20.0;let sample=comparison.sample(time);
            let points=race::poses(comparison,&sample,&runtime);
            let origin=points[comparison.focus(sample.phase)];
            let (cam,_)=race::camera(comparison,&sample,&runtime,Some(&mut collision),time,[1280,720],None);
            use jka_movement::TraceWorld;
            let position=jka_render_core::coordinates::jka_position(cam.position.to_array());
            // Camera travel beyond the runner's own: a snap shows up here.
            let offset=glam::Vec3::from_array(position)-origin;
            if let Some(p)=previous {largest=largest.max(offset.distance(p));}
            previous=Some(offset);
            assert_eq!(collision.point_contents(position,-1)&(0x1|0x10|0x1000),0,"camera inside a wall at {time}");
            let sight=collision.trace(jka_movement::TraceQuery {start:position,end:(origin+glam::Vec3::Z*22.0).to_array(),
                mins:[0.0;3],maxs:[0.0;3],pass_entity:-1,mask:0x1|0x10|0x1000});
            assert!(sight.start_solid==0&&sight.fraction>0.995,"reference hidden by map at {time}");
            for height in [-24.0,64.0] {
                let body=glam::Vec3::from_array(jka_render_core::coordinates::render_position((origin+glam::Vec3::Z*height).to_array()));
                let screen=cam.project_to_screen(1280,720,body).expect("body in front");
                assert!((102.0..1178.0).contains(&screen.x)&&(57.0..663.0).contains(&screen.y),"out of frame at {time:.2}s {:?}: {screen:?}",sample.phase.kind);
            }
            let mut ui=crate::ui::UiSnapshot::default();race::hud(comparison,&sample,&runtime,&cam,[1280,720],None,time,&mut ui);
            assert_eq!(ui.studio_race.unwrap().runners.len(),9);
            for (index,r) in runtime.iter_mut().enumerate() {
                assert!(r.ghost.as_mut().unwrap().sample(1,true,sample.clocks[index].saturating_add(1)).is_some(),"missing {} at {time}",runs[index].name);
            }
        }
        eprintln!("Largest camera offset change per 50 ms: {largest:.1} units");
    }
}
