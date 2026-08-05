//! Command-line argument parsing and site resolution (S2-W4 §6.4).
//! Hand-rolled over `std::env::args_os` — one positional argument and a
//! couple of flags do not justify `clap`. Binary-only: this is about
//! `main`'s own argument handling, not reusable library API.

use std::ffi::OsString;
use std::path::PathBuf;

use radar_workstation::sites::{self, Site};

pub struct Args {
    pub site: Option<String>,
    pub config_path: Option<PathBuf>,
}

pub enum ParseOutcome {
    Args(Args),
    Help,
    Version,
    Error(String),
}

/// `radar-workstation [SITE] [--config PATH] [--help] [--version]`. `SITE`
/// is the one positional argument; order relative to the flags doesn't
/// matter.
pub fn parse<I, T>(args: I) -> ParseOutcome
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    let mut iter = args.into_iter().map(Into::into);
    iter.next(); // argv[0]

    let mut site = None;
    let mut config_path = None;

    while let Some(arg) = iter.next() {
        let arg_str = arg.to_string_lossy().into_owned();
        match arg_str.as_str() {
            "--help" | "-h" => return ParseOutcome::Help,
            "--version" | "-V" => return ParseOutcome::Version,
            "--config" => match iter.next() {
                Some(path) => config_path = Some(PathBuf::from(path)),
                None => return ParseOutcome::Error("--config requires a PATH argument".to_string()),
            },
            _ if arg_str.starts_with('-') && arg_str != "-" => {
                return ParseOutcome::Error(format!("unrecognized option: {arg_str}"));
            }
            _ if site.is_some() => {
                return ParseOutcome::Error(format!("unexpected extra argument: {arg_str}"));
            }
            _ => site = Some(arg_str),
        }
    }

    ParseOutcome::Args(Args { site, config_path })
}

#[derive(Debug, PartialEq, Eq)]
pub enum SiteResolutionError {
    NoSiteSpecified,
    UnknownCliSite(String),
}

/// CLI argument -> config `site` -> error (§6.4). Defaulting to some
/// arbitrary site instead of erroring would start a network connection to
/// a site the user never asked for, which sits badly against BC-1. An
/// unknown *CLI* site is always an error, even if the config has a valid
/// default — the user explicitly typed something, and silently falling
/// back past it would be surprising. An unknown *config* site has already
/// been handled by `config::load` (reported, `config.site` is `None`), so
/// it never reaches this function as anything but "no config site."
pub fn resolve_site(
    cli_site: Option<&str>,
    config_site: Option<&'static Site>,
) -> Result<&'static Site, SiteResolutionError> {
    if let Some(id) = cli_site {
        return sites::by_id(id).ok_or_else(|| SiteResolutionError::UnknownCliSite(id.to_string()));
    }
    config_site.ok_or(SiteResolutionError::NoSiteSpecified)
}

pub fn print_usage() {
    eprintln!("usage: radar-workstation [SITE] [--config PATH] [--help] [--version]");
    eprintln!("example site IDs: KDOX, KTLH, KABR, KAMA");
}

pub fn print_help() {
    println!("radar-workstation — single-site NEXRAD Level II radar analysis");
    println!();
    println!("usage: radar-workstation [SITE] [--config PATH] [--help] [--version]");
    println!();
    println!("  SITE            ICAO site identifier (e.g. KDOX). Overrides the");
    println!("                  configured default site for this run only.");
    println!("  --config PATH   Use PATH instead of the default config file location.");
    println!("  --help, -h      Print this message and exit.");
    println!("  --version, -V   Print the version and exit.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> ParseOutcome {
        parse(std::iter::once("radar-workstation").chain(args.iter().copied()))
    }

    #[test]
    fn no_arguments_yields_no_site_and_no_config_path() {
        match parse_args(&[]) {
            ParseOutcome::Args(args) => {
                assert!(args.site.is_none());
                assert!(args.config_path.is_none());
            }
            _ => panic!("expected Args"),
        }
    }

    #[test]
    fn positional_argument_is_the_site() {
        match parse_args(&["KDOX"]) {
            ParseOutcome::Args(args) => assert_eq!(args.site.as_deref(), Some("KDOX")),
            _ => panic!("expected Args"),
        }
    }

    #[test]
    fn config_flag_takes_a_path_argument() {
        match parse_args(&["--config", "/etc/radar/config"]) {
            ParseOutcome::Args(args) => assert_eq!(args.config_path, Some(PathBuf::from("/etc/radar/config"))),
            _ => panic!("expected Args"),
        }
    }

    #[test]
    fn site_and_config_flag_can_appear_in_either_order() {
        match parse_args(&["--config", "/etc/radar/config", "KTLH"]) {
            ParseOutcome::Args(args) => {
                assert_eq!(args.site.as_deref(), Some("KTLH"));
                assert_eq!(args.config_path, Some(PathBuf::from("/etc/radar/config")));
            }
            _ => panic!("expected Args"),
        }
    }

    #[test]
    fn help_flag_short_circuits() {
        assert!(matches!(parse_args(&["--help"]), ParseOutcome::Help));
        assert!(matches!(parse_args(&["-h"]), ParseOutcome::Help));
        assert!(matches!(parse_args(&["KDOX", "--help"]), ParseOutcome::Help));
    }

    #[test]
    fn version_flag_short_circuits() {
        assert!(matches!(parse_args(&["--version"]), ParseOutcome::Version));
        assert!(matches!(parse_args(&["-V"]), ParseOutcome::Version));
    }

    #[test]
    fn config_flag_with_no_path_is_an_error() {
        assert!(matches!(parse_args(&["--config"]), ParseOutcome::Error(_)));
    }

    #[test]
    fn unrecognized_flag_is_an_error() {
        assert!(matches!(parse_args(&["--bogus"]), ParseOutcome::Error(_)));
    }

    #[test]
    fn a_second_positional_argument_is_an_error() {
        assert!(matches!(parse_args(&["KDOX", "KTLH"]), ParseOutcome::Error(_)));
    }

    #[test]
    fn resolve_site_prefers_cli_argument_over_config() {
        let kdox = sites::by_id("KDOX").unwrap();
        let ktlh = sites::by_id("KTLH").unwrap();
        assert_eq!(resolve_site(Some("KTLH"), Some(kdox)), Ok(ktlh));
    }

    #[test]
    fn resolve_site_falls_back_to_config_when_no_cli_argument() {
        let kdox = sites::by_id("KDOX").unwrap();
        assert_eq!(resolve_site(None, Some(kdox)), Ok(kdox));
    }

    #[test]
    fn resolve_site_errors_when_neither_is_present() {
        assert_eq!(resolve_site(None, None), Err(SiteResolutionError::NoSiteSpecified));
    }

    #[test]
    fn resolve_site_errors_on_an_unknown_cli_site_even_with_a_valid_config_default() {
        let kdox = sites::by_id("KDOX").unwrap();
        assert_eq!(resolve_site(Some("ZZZZ"), Some(kdox)), Err(SiteResolutionError::UnknownCliSite("ZZZZ".to_string())));
    }
}
