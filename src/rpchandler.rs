use std::collections::HashMap;
use std::time::Duration;

use discord_rich_presence::activity::{Assets, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity::Activity};
use tokio::sync::mpsc::Receiver;
use tokio::time::sleep;

use crate::CLIENT_ID;
use crate::watcher::EbuildJob;

pub(crate) struct RPCHandler {
    /// sender for updates
    rx: Receiver<Vec<EbuildJob>>,
}

impl RPCHandler {
    /// create new RPCHandler
    pub(crate) fn new(rx: Receiver<Vec<EbuildJob>>) -> Self {
        Self { rx }
    }

    /// start listening and sending updates
    pub(crate) async fn start(mut self) -> Result<(), String> {
        let mut client = match DiscordIpcClient::new(CLIENT_ID) {
            Ok(client) => client,
            Err(e) => return Err(e.to_string()),
        };

        // stupid lazy Box<dyn std::error::Error>> is not Send....
        while let Err(e) = client.connect().map_err(|e| e.to_string()) {
            eprintln!("Connecting to Discord failed: {}", e);
            eprintln!("Retrying in 5 seconds");
            sleep(Duration::from_secs(5)).await;
        }

        println!("Connected to Discord");

        let mut cleared = true;
        while let Some(jobs) = self.rx.recv().await {
            #[cfg(debug_assertions)]
            println!("Handler received update");

            // clear on empty set
            if jobs.is_empty() {
                // don't clear multiple times
                if cleared {
                    continue;
                }

                if let Err(e) = client.clear_activity() {
                    eprintln!("Error clearing activity: {}", e);
                } else {
                    cleared = true;
                }
                continue;
            }
            cleared = false;

            // first line
            let first = match jobs.len() {
                1 => format!(
                    "{}/{}-{}",
                    jobs[0].category, jobs[0].package, jobs[0].version
                ),
                _ => format!("Running {} Jobs", jobs.len()),
            };

            // second line
            let second = match jobs.len() {
                1 => format!("Phase: {}", jobs[0].phase),
                _ => {
                    let mut counter: HashMap<String, u32> = HashMap::new();
                    for job in &jobs {
                        match counter.get(&job.phase) {
                            Some(curr) => {
                                counter.insert(job.phase.clone(), curr + 1);
                            }
                            None => {
                                counter.insert(job.phase.clone(), 1);
                            }
                        }
                    }
                    let mut phases_vec = Vec::new();
                    for (phase, count) in counter {
                        if count > 0 {
                            phases_vec.push(format!("{} ({})", phase, count));
                        }
                    }
                    format!("Phases: {}", phases_vec.join(", "))
                }
            };

            // timestamp
            let start_time = match jobs.len() {
                1 => jobs[0].create_time.as_secs(),
                _ => {
                    let mut min: u64 = jobs[0].create_time.as_secs();
                    for job in &jobs[1..] {
                        let this = job.create_time.as_secs();
                        if this < min {
                            min = this;
                        }
                    }
                    min
                }
            };

            #[cfg(debug_assertions)]
            println!(
                "Sending update: state=\"{}\", details=\"{}\", start_time=\"{}\"",
                &second, &first, &start_time
            );

            if let Err(e) = client.set_activity(
                Activity::new()
                    .state(&second)
                    .details(&first)
                    .timestamps(Timestamps::new().start(start_time as i64))
                    .assets(Assets::new().large_image("gentoo_box")),
            ) {
                eprintln!("Error setting activity: {}", e);
            }
        }

        Err(String::from("Connection to process watcher died"))
    }
}
