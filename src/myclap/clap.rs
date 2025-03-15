use clap::{
    builder::Styles,
    error::{ContextKind, ContextValue, ErrorKind},
};
const TAB: &str = "  ";

pub struct MyErrorFormatter;

impl clap::error::ErrorFormatter for MyErrorFormatter {
    fn format_error(error: &clap::error::Error<Self>) -> clap::builder::StyledStr {
        use std::fmt::Write as _;
        let styles = Styles::default();
        let mut styled = String::new();
        let valid = &styles.get_valid();
        let _ = write!(styled, "错误:");

        if !write_dynamic_context(error, &mut styled, &styles) {
            if let Some(msg) = error.kind().as_str() {
                styled.push_str(msg);
            } else {
                styled.push_str("未知原因");
            }
        }

        let mut suggested = false;
        if let Some(valid) = error.get(ContextKind::SuggestedSubcommand) {
            styled.push_str("\n");
            if !suggested {
                styled.push_str("\n");
                suggested = true;
            }
            did_you_mean(&mut styled, &styles, "子命令", valid);
        }
        if let Some(valid) = error.get(ContextKind::SuggestedArg) {
            styled.push_str("\n");
            if !suggested {
                styled.push_str("\n");
                suggested = true;
            }
            did_you_mean(&mut styled, &styles, "参数", valid);
        }
        if let Some(valid) = error.get(ContextKind::SuggestedValue) {
            styled.push_str("\n");
            if !suggested {
                styled.push_str("\n");
                suggested = true;
            }
            did_you_mean(&mut styled, &styles, "值", valid);
        }
        let suggestions = error.get(ContextKind::Suggested);
        if let Some(ContextValue::StyledStrs(suggestions)) = suggestions {
            if !suggested {
                styled.push_str("\n");
            }
            for suggestion in suggestions {
                let _ = write!(styled, "\n{TAB}{valid}提示:{valid:#} ",);
                styled.push_str(&suggestion.to_string());
            }
        }
        clap::builder::StyledStr::from(styled)
    }
}

fn write_dynamic_context(
    error: &clap::error::Error<MyErrorFormatter>,
    styled: &mut String,
    styles: &Styles,
) -> bool {
    use std::fmt::Write as _;
    let valid = styles.get_valid();
    let invalid = styles.get_invalid();
    let literal = styles.get_literal();

    match error.kind() {
        ErrorKind::ArgumentConflict => {
            let mut prior_arg = error.get(ContextKind::PriorArg);
            if let Some(ContextValue::String(invalid_arg)) = error.get(ContextKind::InvalidArg) {
                if Some(&ContextValue::String(invalid_arg.clone())) == prior_arg {
                    prior_arg = None;
                    let _ = write!(
                        styled,
                        "参数 '{invalid}{invalid_arg}{invalid:#}' 不能多次使用",
                    );
                } else {
                    let _ = write!(
                        styled,
                        "参数 '{invalid}{invalid_arg}{invalid:#}' 不能与以下内容一起使用",
                    );
                }
            } else if let Some(ContextValue::String(invalid_arg)) =
                error.get(ContextKind::InvalidSubcommand)
            {
                let _ = write!(
                    styled,
                    "子命令 '{invalid}{invalid_arg}{invalid:#}' 不能与以下内容一起使用",
                );
            } else {
                styled.push_str(error.kind().as_str().unwrap());
            }

            if let Some(prior_arg) = prior_arg {
                match prior_arg {
                    ContextValue::Strings(values) => {
                        styled.push_str(":");
                        for v in values {
                            let _ = write!(styled, "\n{TAB}{invalid}{v}{invalid:#}",);
                        }
                    }
                    ContextValue::String(value) => {
                        let _ = write!(styled, " '{invalid}{value}{invalid:#}'",);
                    }
                    _ => {
                        styled.push_str(" 一个或多个指定的参数");
                    }
                }
            }

            true
        }
        ErrorKind::NoEquals => {
            let invalid_arg = error.get(ContextKind::InvalidArg);
            if let Some(ContextValue::String(invalid_arg)) = invalid_arg {
                let _ = write!(
                    styled,
                    "为 '{invalid}{invalid_arg}{invalid:#}' 分配值时需要等号",
                );
                true
            } else {
                false
            }
        }
        ErrorKind::InvalidValue => {
            let invalid_arg = error.get(ContextKind::InvalidArg);
            let invalid_value = error.get(ContextKind::InvalidValue);
            if let (
                Some(ContextValue::String(invalid_arg)),
                Some(ContextValue::String(invalid_value)),
            ) = (invalid_arg, invalid_value)
            {
                if invalid_value.is_empty() {
                    let _ = write!(
                        styled,
                        "'{invalid}{invalid_arg}{invalid:#}' 需要一个值，但未提供",
                    );
                } else {
                    let _ = write!(
                        styled,
                        "'{literal}{invalid_arg}{literal:#}' 的值 '{invalid}{invalid_value}{invalid:#}' 无效",
                    );
                }

                let values = error.get(ContextKind::ValidValue);
                write_values_list("可能的值", styled, valid, values);

                true
            } else {
                false
            }
        }
        ErrorKind::InvalidSubcommand => {
            let invalid_sub = error.get(ContextKind::InvalidSubcommand);
            if let Some(ContextValue::String(invalid_sub)) = invalid_sub {
                let _ = write!(
                    styled,
                    "无法识别的子命令 '{invalid}{invalid_sub}{invalid:#}'",
                );
                true
            } else {
                false
            }
        }
        ErrorKind::MissingRequiredArgument => {
            let invalid_arg = error.get(ContextKind::InvalidArg);
            if let Some(ContextValue::Strings(invalid_arg)) = invalid_arg {
                styled.push_str("以下必需参数未提供：");
                for v in invalid_arg {
                    let _ = write!(styled, "\n{TAB}{valid}{v}{valid:#}",);
                }
                true
            } else {
                false
            }
        }
        ErrorKind::MissingSubcommand => {
            let invalid_sub = error.get(ContextKind::InvalidSubcommand);
            if let Some(ContextValue::String(invalid_sub)) = invalid_sub {
                let _ = write!(
                    styled,
                    "'{invalid}{invalid_sub}{invalid:#}' 需要一个子命令，但未提供",
                );
                let values = error.get(ContextKind::ValidSubcommand);
                write_values_list("子命令", styled, valid, values);

                true
            } else {
                false
            }
        }
        ErrorKind::InvalidUtf8 => false,
        ErrorKind::TooManyValues => {
            let invalid_arg = error.get(ContextKind::InvalidArg);
            let invalid_value = error.get(ContextKind::InvalidValue);
            if let (
                Some(ContextValue::String(invalid_arg)),
                Some(ContextValue::String(invalid_value)),
            ) = (invalid_arg, invalid_value)
            {
                let _ = write!(
                    styled,
                    "发现 '{literal}{invalid_arg}{literal:#}' 的意外值 '{invalid}{invalid_value}{invalid:#}'；不需要更多值",
                );
                true
            } else {
                false
            }
        }
        ErrorKind::TooFewValues => {
            let invalid_arg = error.get(ContextKind::InvalidArg);
            let actual_num_values = error.get(ContextKind::ActualNumValues);
            let min_values = error.get(ContextKind::MinValues);
            if let (
                Some(ContextValue::String(invalid_arg)),
                Some(ContextValue::Number(actual_num_values)),
                Some(ContextValue::Number(min_values)),
            ) = (invalid_arg, actual_num_values, min_values)
            {
                let were_provided = singular_or_plural(*actual_num_values as usize);
                let _ = write!(
                    styled,
                    "'{literal}{invalid_arg}{literal:#}' 需要 {valid}{min_values}{valid:#} 个值，但只提供了 {invalid}{actual_num_values}{invalid:#}{were_provided}",
                );
                true
            } else {
                false
            }
        }
        ErrorKind::ValueValidation => {
            let invalid_arg = error.get(ContextKind::InvalidArg);
            let invalid_value = error.get(ContextKind::InvalidValue);
            if let (
                Some(ContextValue::String(invalid_arg)),
                Some(ContextValue::String(invalid_value)),
            ) = (invalid_arg, invalid_value)
            {
                let _ = write!(
                    styled,
                    "'{literal}{invalid_arg}{literal:#}' 的值 '{invalid}{invalid_value}{invalid:#}' 无效",
                );
                true
            } else {
                false
            }
        }
        ErrorKind::WrongNumberOfValues => {
            let invalid_arg = error.get(ContextKind::InvalidArg);
            let actual_num_values = error.get(ContextKind::ActualNumValues);
            let num_values = error.get(ContextKind::ExpectedNumValues);
            if let (
                Some(ContextValue::String(invalid_arg)),
                Some(ContextValue::Number(actual_num_values)),
                Some(ContextValue::Number(num_values)),
            ) = (invalid_arg, actual_num_values, num_values)
            {
                let were_provided = singular_or_plural(*actual_num_values as usize);
                let _ = write!(
                    styled,
                    "'{literal}{invalid_arg}{literal:#}' 需要 {valid}{num_values}{valid:#} 个值，但提供了 {invalid}{actual_num_values}{invalid:#}{were_provided}",
                );
                true
            } else {
                false
            }
        }
        ErrorKind::UnknownArgument => {
            let invalid_arg = error.get(ContextKind::InvalidArg);
            if let Some(ContextValue::String(invalid_arg)) = invalid_arg {
                let _ = write!(styled, "发现意外参数 '{invalid}{invalid_arg}{invalid:#}'",);
                true
            } else {
                false
            }
        }
        ErrorKind::DisplayHelp
        | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        | ErrorKind::DisplayVersion
        | ErrorKind::Io
        | ErrorKind::Format
        | _ => false,
    }
}

fn singular_or_plural(n: usize) -> &'static str {
    if n > 1 { " 个" } else { " 个" }
}

fn write_values_list(
    list_name: &'static str,
    styled: &mut String,
    valid: &anstyle::Style,
    possible_values: Option<&ContextValue>,
) {
    use std::fmt::Write as _;
    if let Some(ContextValue::Strings(possible_values)) = possible_values {
        if !possible_values.is_empty() {
            let _ = write!(styled, "\n{TAB}[{list_name}: ");

            for (idx, val) in possible_values.iter().enumerate() {
                if idx > 0 {
                    styled.push_str(", ");
                }
                let _ = write!(styled, "{valid}{}{valid:#}", Escape(val));
            }

            styled.push_str("]");
        }
    }
}

struct Escape<'s>(&'s str);

impl std::fmt::Display for Escape<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.contains(char::is_whitespace) {
            std::fmt::Debug::fmt(self.0, f)
        } else {
            self.0.fmt(f)
        }
    }
}

fn did_you_mean(styled: &mut String, styles: &Styles, context: &str, possibles: &ContextValue) {
    use std::fmt::Write as _;

    let valid = &styles.get_valid();
    let _ = write!(styled, "{TAB}{valid}提示:{valid:#}",);
    if let ContextValue::String(possible) = possibles {
        let _ = write!(styled, " 存在类似的{context}: '{valid}{possible}{valid:#}'",);
    } else if let ContextValue::Strings(possibles) = possibles {
        if possibles.len() == 1 {
            let _ = write!(styled, " 存在类似的{context}: ",);
        } else {
            let _ = write!(styled, " 存在一些类似的{context}: ",);
        }
        for (i, possible) in possibles.iter().enumerate() {
            if i != 0 {
                styled.push_str(", ");
            }
            let _ = write!(styled, "'{valid}{possible}{valid:#}'",);
        }
    }
}
