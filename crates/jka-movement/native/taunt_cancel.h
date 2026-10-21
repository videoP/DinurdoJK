/* Shared native full-body taunt interruption rule. BUTTON_* come from q_shared.h. */
#ifndef JKA_TAUNT_CANCEL_H
#define JKA_TAUNT_CANCEL_H
static int jka_taunt_cancel_input(int buttons, int upmove) {
    return (buttons & (BUTTON_ATTACK | BUTTON_ALT_ATTACK | BUTTON_FORCEPOWER |
                      BUTTON_FORCEGRIP | BUTTON_FORCE_LIGHTNING | BUTTON_FORCE_DRAIN))
        || upmove;
}
#endif
