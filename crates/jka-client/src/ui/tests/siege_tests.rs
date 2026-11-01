use super::*;

#[test]
fn siege_scoreboard_icons_share_the_atlas_with_team_and_duel_huds() {
    let mut ui = UiSnapshot::default();
    ui.team_overlay = Some(TeamOverlayUi {
        settings: TeamOverlaySettings::default(),
        team: 1,
        location_width: 0,
        has_locations: false,
        icon_paths: vec!["team_a".into(), "team_b".into()],
        entries: Vec::new(),
    });
    let duelist = UiDuelMiniScore {
        name: "Duelist".into(),
        score: Some(0),
        model_icon: Some(0),
    };
    ui.mini_scores = Some(UiMiniScores::Duel {
        icon_paths: vec!["duel_a".into(), "duel_b".into()],
        blue: duelist.clone(),
        red: duelist,
    });
    ui.scoreboard = Some(UiScoreboard {
        team_scores: [0; 2],
        ready_clients: 0,
        team_game: true,
        ctf: false,
        siege: true,
        siege_win_team: 0,
        spectator_scores: false,
        show_deaths: false,
        icon_paths: vec!["siege_class".into()],
        entries: Vec::new(),
        chrome: Default::default(),
    });
    assert_eq!(shared_player_icon_paths(&ui), ["team_a", "team_b", "duel_a", "duel_b", "siege_class"]);
    assert_eq!(duel_icon_atlas_offset(&ui), 2);
    assert_eq!(scoreboard_icon_atlas_offset(&ui), 4);
    assert_eq!(shared_player_icon_count(&ui), 5);
    ui.team_overlay = None;
    assert_eq!(scoreboard_icon_atlas_offset(&ui), 2);
    ui.mini_scores = None;
    assert_eq!(scoreboard_icon_atlas_offset(&ui), 0);
    assert_eq!(shared_player_icon_paths(&ui), ["siege_class"]);
}
