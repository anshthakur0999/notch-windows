//! Command line: `notch.exe` starts the pill; `notch.exe hide|show|toggle|quit` talks to the
//! already-running instance.

/// Command codes carried in `WM_APP_COMMAND`'s `wParam`.
pub const CMD_HIDE: usize = 1;
pub const CMD_SHOW: usize = 2;
pub const CMD_TOGGLE: usize = 3;
pub const CMD_QUIT: usize = 4;
/// Sent in `WM_APP_COMMAND`'s `lParam`. `WM_APP + n` is a plain number that any program may post
/// to a top-level window, so a command only counts when it carries this tag ("NOTC").
pub const CMD_MAGIC: isize = 0x4E4F_5443;

pub const USAGE: &str = "Notch - floating pill with notepad and clipboard history

Usage:
  notch.exe              start the pill
  notch.exe --hidden     start with the pill hidden
  notch.exe hide         hide the pill (clipboard capture keeps running)
  notch.exe show         show the pill again
  notch.exe toggle       hide it if visible, show it if hidden
  notch.exe quit         close the running Notch
  notch.exe help         this text";

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    /// Run the pill itself.
    Run { start_hidden: bool },
    /// Send a command to the running instance.
    Send(usize),
    Help,
    Unknown(String),
}

/// Parses the arguments after the program name. Leading `-`, `--` or `/` are optional.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Action {
    let Some(first) = args.into_iter().next() else {
        return Action::Run { start_hidden: false };
    };
    let word = first.trim_start_matches(['-', '/']).to_ascii_lowercase();
    match word.as_str() {
        "hide" => Action::Send(CMD_HIDE),
        "show" | "unhide" => Action::Send(CMD_SHOW),
        "toggle" => Action::Send(CMD_TOGGLE),
        "quit" | "exit" | "stop" => Action::Send(CMD_QUIT),
        "hidden" | "start-hidden" => Action::Run { start_hidden: true },
        "help" | "h" | "?" => Action::Help,
        _ => Action::Unknown(first),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(args: &[&str]) -> Action {
        parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_arguments_starts_the_pill() {
        assert_eq!(p(&[]), Action::Run { start_hidden: false });
    }

    #[test]
    fn commands_accept_any_dash_style_and_case() {
        for form in ["hide", "--hide", "-hide", "/hide", "HIDE", "--Hide"] {
            assert_eq!(p(&[form]), Action::Send(CMD_HIDE), "{form}");
        }
        assert_eq!(p(&["show"]), Action::Send(CMD_SHOW));
        assert_eq!(p(&["unhide"]), Action::Send(CMD_SHOW));
        assert_eq!(p(&["--toggle"]), Action::Send(CMD_TOGGLE));
        assert_eq!(p(&["quit"]), Action::Send(CMD_QUIT));
        assert_eq!(p(&["exit"]), Action::Send(CMD_QUIT));
    }

    #[test]
    fn hidden_flag_starts_hidden() {
        assert_eq!(p(&["--hidden"]), Action::Run { start_hidden: true });
        assert_eq!(p(&["--start-hidden"]), Action::Run { start_hidden: true });
    }

    #[test]
    fn help_and_unknown() {
        assert_eq!(p(&["--help"]), Action::Help);
        assert_eq!(p(&["?"]), Action::Help);
        assert_eq!(p(&["--banana"]), Action::Unknown("--banana".into()));
    }

    #[test]
    fn only_the_first_argument_counts() {
        assert_eq!(p(&["hide", "show"]), Action::Send(CMD_HIDE));
    }
}
