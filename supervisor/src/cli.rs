#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    Serve,
    Service(ServiceAction),
    Platform(PlatformAction),
    DoctorJson,
}

#[derive(Debug, PartialEq, Eq)]
pub enum PlatformAction {
    Configure,
    ConfigInit,
    UpgradeInit,
    Install,
    Upgrade,
    Uninstall,
    MirrorGet,
    MirrorSet(Option<String>),
}

#[derive(Debug, PartialEq, Eq)]
pub enum ServiceAction {
    Start,
    Stop,
    Restart,
    Status,
}

pub fn parse(args: &[String]) -> Result<Command, String> {
    match args {
        [] => Ok(Command::Serve),
        [arg] if arg == "serve" => Ok(Command::Serve),
        [command, action] if command == "service" => {
            let action = match action.as_str() {
                "start" => ServiceAction::Start,
                "stop" => ServiceAction::Stop,
                "restart" => ServiceAction::Restart,
                "status" => ServiceAction::Status,
                _ => return Err(format!("unknown service action: {action}")),
            };
            Ok(Command::Service(action))
        }
        [command, action] if command == "platform" => {
            let action = match action.as_str() {
                "configure" => PlatformAction::Configure,
                "config-init" => PlatformAction::ConfigInit,
                "upgrade-init" => PlatformAction::UpgradeInit,
                "install" => PlatformAction::Install,
                "upgrade" => PlatformAction::Upgrade,
                "uninstall" => PlatformAction::Uninstall,
                "mirror-get" => PlatformAction::MirrorGet,
                "mirror-set" => PlatformAction::MirrorSet(None),
                _ => return Err(format!("unknown platform action: {action}")),
            };
            Ok(Command::Platform(action))
        }
        [command, action, value] if command == "platform" && action == "mirror-set" => {
            Ok(Command::Platform(PlatformAction::MirrorSet(Some(
                value.clone(),
            ))))
        }
        [command, flag] if command == "doctor" && flag == "--json" => Ok(Command::DoctorJson),
        _ => Err(
            "usage: qwenpaw-supervisor [serve | service start|stop|restart|status | platform install|upgrade|uninstall|configure|config-init|upgrade-init|mirror-get|mirror-set <value> | doctor --json]"
                .into(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command, String> {
        parse(
            &args
                .iter()
                .map(|arg| (*arg).to_string())
                .collect::<Vec<_>>(),
        )
    }

    #[test]
    fn accepts_default_and_explicit_serve() {
        assert_eq!(parse_args(&[]), Ok(Command::Serve));
        assert_eq!(parse_args(&["serve"]), Ok(Command::Serve));
    }

    #[test]
    fn rejects_retired_web_cgi_commands() {
        assert!(parse_args(&["web", "api-cgi"]).is_err());
        assert!(parse_args(&["web", "index-cgi"]).is_err());
        assert!(parse_args(&["web", "unknown"]).is_err());
    }

    #[test]
    fn parses_native_service_actions() {
        assert_eq!(
            parse_args(&["service", "start"]),
            Ok(Command::Service(ServiceAction::Start))
        );
        assert_eq!(
            parse_args(&["service", "stop"]),
            Ok(Command::Service(ServiceAction::Stop))
        );
        assert_eq!(
            parse_args(&["service", "restart"]),
            Ok(Command::Service(ServiceAction::Restart))
        );
        assert_eq!(
            parse_args(&["service", "status"]),
            Ok(Command::Service(ServiceAction::Status))
        );
    }

    #[test]
    fn accepts_only_json_doctor_and_rejects_unknown_arguments() {
        assert_eq!(parse_args(&["doctor", "--json"]), Ok(Command::DoctorJson));
        assert!(parse_args(&["doctor"]).is_err());
        assert!(parse_args(&["service", "reload"]).is_err());
        assert!(parse_args(&["serve", "extra"]).is_err());
        assert!(parse_args(&["unknown"]).is_err());
    }

    #[test]
    fn parses_each_native_platform_action() {
        assert_eq!(
            parse_args(&["platform", "configure"]),
            Ok(Command::Platform(PlatformAction::Configure))
        );
        assert_eq!(
            parse_args(&["platform", "config-init"]),
            Ok(Command::Platform(PlatformAction::ConfigInit))
        );
        assert_eq!(
            parse_args(&["platform", "upgrade-init"]),
            Ok(Command::Platform(PlatformAction::UpgradeInit))
        );
        for action in ["install", "upgrade", "uninstall"] {
            assert!(
                parse_args(&["platform", action]).is_ok(),
                "platform {action} must be available"
            );
        }
    }

    #[test]
    fn accepts_platform_mirror_commands_and_distinguishes_empty_from_missing() {
        assert_eq!(
            parse_args(&["platform", "mirror-get"]),
            Ok(Command::Platform(PlatformAction::MirrorGet))
        );
        assert_eq!(
            parse_args(&["platform", "mirror-set", ""]),
            Ok(Command::Platform(PlatformAction::MirrorSet(Some(
                String::new()
            ))))
        );
        assert_eq!(
            parse_args(&["platform", "mirror-set"]),
            Ok(Command::Platform(PlatformAction::MirrorSet(None)))
        );
        assert_eq!(
            parse_args(&["platform", "mirror-set", "https://mirror.example/simple"]),
            Ok(Command::Platform(PlatformAction::MirrorSet(Some(
                "https://mirror.example/simple".into()
            ))))
        );
        assert!(parse_args(&["platform", "mirror-set", "value", "extra"]).is_err());
    }
}
