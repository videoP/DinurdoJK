//! OpenJK/TaystJK kbutton commands.
use crate::app::App;
use super::{CommandDef, CommandResult, CommandScope};

commands! {
    cmd_input_button [
        "+moveup", "-moveup",
        "+movedown", "-movedown",
        "+forward", "-forward",
        "+back", "-back",
        "+moveleft", "-moveleft",
        "+moveright", "-moveright",
        "+speed", "-speed",
        "+attack", "-attack",
        "+use", "-use",
        "+force_grip", "-force_grip",
        "+altattack", "-altattack",
        "+useforce", "-useforce",
        "+force_lightning", "-force_lightning",
        "+force_drain", "-force_drain",
        "+button0", "-button0",
        "+button1", "-button1",
        "+button2", "-button2",
        "+button3", "-button3",
        "+button4", "-button4",
        "+button5", "-button5",
        "+button6", "-button6",
        "+button7", "-button7",
        "+button8", "-button8",
        "+button9", "-button9",
        "+button10", "-button10",
        "+button11", "-button11",
        "+button12", "-button12",
        "+button13", "-button13",
        "+button14", "-button14",
        "+button15", "-button15",
        "+zoom", "-zoom",
    ] (
        "+command / -command",
        "Set or clear a held client input button.",
        CommandScope::Always
    ) |self, command| {
        if self.execute_input_button_command(command) {
            CommandResult::Handled
        } else {
            CommandResult::Unhandled
        }
    }
}
