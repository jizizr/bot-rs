#[macro_use]
pub mod common;

simple_command_test!(start, bot_rs::funcs::command::start::start);
simple_command_test!(my, bot_rs::funcs::command::quote::quote);
simple_command_test!(id, bot_rs::funcs::command::id::id);
simple_command_test!(today, bot_rs::funcs::command::today::today);
simple_command_test!(rate, bot_rs::funcs::command::rate::rate);
simple_command_test!(short, bot_rs::funcs::command::short::short);
simple_command_test!(wiki, bot_rs::funcs::command::wiki::wiki);
simple_command_test!(translate, bot_rs::funcs::command::translate::translate);
simple_command_test!(wcloud, bot_rs::funcs::command::wcloud::wcloud);
simple_command_test!(music, bot_rs::funcs::command::music::music);
simple_command_test!(ping, bot_rs::funcs::command::ping::ping);
simple_command_test!(config, bot_rs::funcs::command::config::config);

