extern crate docopt;
extern crate futures;
extern crate hlua;
extern crate hyper;
extern crate hyper_native_tls;
extern crate iron;
#[macro_use]
extern crate lazy_static;
#[macro_use]
extern crate log;
extern crate lru_time_cache;
extern crate regex;
extern crate router;
extern crate rusoto_core;
extern crate rusoto_sqs;
extern crate serde;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate serde_json;
extern crate ssh2;
extern crate stderrlog;
extern crate tokio_core;

use futures::Stream;
use std::time::Duration;

#[macro_use]
mod utils;
mod args;
mod bot;
mod gerrit;
mod spark;
mod sqs;

fn main() {
    let args = args::parse_args();
    stderrlog::new()
        .module(module_path!())
        .quiet(args.flag_quiet)
        .timestamp(stderrlog::Timestamp::Second)
        .verbosity(if args.flag_verbose {5} else {2})
        .init()
        .unwrap();
    info!("Starting");
    debug!("Arguments: {:?}", args);

    // load or create a new bot
    let mut bot = match bot::Bot::load("state.json") {
        Ok(bot) => {
            info!(
                "Loaded bot from 'state.json' with {} user(s).",
                bot.num_users()
            );
            bot
        }
        Err(err) => {
            warn!("Could not load bot from 'state.json': {:?}", err);
            bot::Bot::new()
        }
    };
    if args.flag_bot_msg_expiration != 0 && args.flag_bot_msg_capacity != 0 {
        debug!(
            "Approval LRU cache: capacity - {}, expiration - {} sec",
            args.flag_bot_msg_capacity,
            args.flag_bot_msg_expiration
        );
        bot.init_msg_cache(args.flag_bot_msg_capacity, Duration::from_secs(args.flag_bot_msg_expiration));
    };

    // event loop
    let mut core = tokio_core::reactor::Core::new().unwrap();

    // create spark client and event stream listener
    let spark_client =
        spark::SparkClient::new(args.flag_spark_url, args.flag_spark_bot_token, args.flag_spark_webhook_url)
            .unwrap_or_else(|err| {
                error!("Could not create spark client: {}", err);
                std::process::exit(1);
            });

    let spark_stream = if !args.flag_spark_sqs.is_empty() {
        spark::sqs_event_stream(spark_client.clone(), args.flag_spark_sqs, args.flag_spark_sqs_region)
    } else {
        spark::webhook_event_stream(spark_client.clone(), &args.flag_spark_endpoint, core.remote())
    };
    let spark_stream = spark_stream.unwrap_or_else(|err| {
        error!("Could not start listening to spark: {}", err);
        std::process::exit(1);
    });

    // create gerrit event stream listener
    let gerrit_stream = gerrit::event_stream(
        &args.flag_gerrit_hostname,
        args.flag_gerrit_port,
        args.flag_gerrit_username,
        args.flag_gerrit_priv_key_path,
    );

    // join spark and gerrit action stream into one and fold over actions with accumulator `bot`
    let handle = core.handle();
    let actions = spark_stream
        .select(gerrit_stream)
        .filter(|action| match *action {
            bot::Action::NoOp => false,
            _ => true,
        })
        .filter_map(|action| {
            debug!("Handle action: {:?}", action);

            // fold over actions
            let old_bot = std::mem::replace(&mut bot, bot::Bot::new());
            let (new_bot, task) = bot::update(action, old_bot);
            std::mem::replace(&mut bot, new_bot);

            // Handle save task and return response.
            // Note: We have to do it here, since the value of `bot` is only
            // available in this function.
            if let Some(task) = task {
                debug!("New task {:?}", task);
                let response = match task {
                    bot::Task::Reply(response) => response,
                    bot::Task::ReplyAndSave(response) => {
                        let bot_clone = bot.clone();
                        handle.spawn_fn(move || {
                            if let Err(err) = bot_clone.save("state.json") {
                                error!("Could not save state: {:?}", err);
                            }
                            Ok(())
                        });
                        response
                    }
                };
                return Some(response);
            }
            None
        })
        .or_else(|err| {
            error!("Exit due to error: {:?}", err);
            Err(())
        })
        .for_each(|response| {
            debug!("Replying with: {}", response.message);
            spark_client.reply(&response.person_id, &response.message);
            Ok(())
        });

    let _ = core.run(actions);
}
