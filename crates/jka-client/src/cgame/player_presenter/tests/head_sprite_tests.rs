use super::head_sprite_shader;

#[test]
fn head_sprite_follows_cg_playersprites_precedence() {
    const TALK: i32 = 1 << 13;
    const CONNECTION: i32 = 1 << 14;
    assert_eq!(head_sprite_shader(0, false), None);
    assert_eq!(head_sprite_shader(TALK, false), Some("gfx/mp/chat_icon"));
    assert_eq!(head_sprite_shader(TALK, true), Some("gfx/mp/vchat_icon"));
    assert_eq!(
        head_sprite_shader(TALK | CONNECTION, true),
        Some("gfx/2d/net")
    );
    assert_eq!(head_sprite_shader(0, true), Some("gfx/mp/vchat_icon"));
}
