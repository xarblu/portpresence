use std::collections::HashMap;
use std::time::Duration;

use discord_rich_presence::activity::{Assets, Timestamps};
use discord_rich_presence::{DiscordIpc, DiscordIpcClient, activity::Activity};
use tokio::sync::mpsc::Receiver;
use tokio::time::sleep;

use crate::CLIENT_ID;
use crate::portage_info::ebuild_version;
use crate::watcher::ActiveJobs;

pub(crate) struct RPCHandler {
    /// sender for updates
    rx: Receiver<ActiveJobs>,
}

impl RPCHandler {
    /// create new RPCHandler
    pub(crate) fn new(rx: Receiver<ActiveJobs>) -> Self {
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
        let mut version_str: Option<String> = None;
        while let Some(job_trees) = self.rx.recv().await {
            #[cfg(debug_assertions)]
            println!("Handler received update");

            // track if we should reconnect
            let mut should_reconnect = false;

            // clear on empty set
            if job_trees.is_empty() {
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

            // first iteration after clearing
            // per-session tasks should go here
            if cleared {
                version_str = match ebuild_version() {
                    Ok(ver) => Some(ver),
                    Err(e) => {
                        eprintln!("Error getting ebuild version: {}", e);
                        None
                    }
                };
                cleared = false;
            }

            // now redefine jobs to a combination of all trees
            let mut jobs = Vec::new();
            for job_tree in job_trees.values() {
                for job in job_tree.values() {
                    jobs.push(job);
                }
            }

            // first line
            let info = match jobs.len() {
                0 => String::from("No Jobs Running"),
                1 => format!(
                    "{}/{}-{}",
                    jobs[0].category, jobs[0].package, jobs[0].version
                ),
                _ => format!("{} Jobs Running", jobs.len()),
            };

            // phase info of running jobs
            let phases: Option<String>;
            let mut phase_icon: Option<&str> = None;
            match jobs.len() {
                0 => phases = None,
                1 => {
                    phases = Some(format!("Phase: {}", jobs[0].phase));
                    phase_icon = match jobs[0].phase.as_str() {
                        "unpack" => Some("phase_unpack"),
                        "prepare" => Some("phase_prepare"),
                        "configure" => Some("phase_configure"),
                        "compile" => Some("phase_compile"),
                        "install" => Some("phase_install"),
                        _ => None,
                    };
                }
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
                    phases = Some(format!("Phases: {}", phases_vec.join(", ")));
                }
            }

            // timestamp
            let start_time = match jobs.len() {
                0 => None,
                1 => Some(jobs[0].create_time.as_secs() as i64),
                _ => {
                    let mut min: i64 = jobs[0].create_time.as_secs() as i64;
                    for job in &jobs[1..] {
                        let this = job.create_time.as_secs() as i64;
                        if this < min {
                            min = this;
                        }
                    }
                    Some(min)
                }
            };

            let mut activity = Activity::new().details(&info);

            // state (2nd line) is None if emerge doesn't have jobs running
            if let Some(ref phases) = phases {
                activity = activity.state(phases);
            }

            // start time is only set if jobs are running
            // I think by default this will use time the activity was set
            if let Some(time) = start_time {
                activity = activity.timestamps(Timestamps::new().start(time));
            }

            // add assets
            let mut assets = Assets::new();
            assets = assets.large_image("gentoo_box");
            if let Some(ref version_str) = version_str {
                assets = assets.large_text(version_str);
            }
            if let Some(phase_icon) = phase_icon {
                assets = assets.small_image(phase_icon);
                if let Some(ref phases) = phases {
                    assets = assets.small_text(phases);
                }
            }
            activity = activity.assets(assets);

            #[cfg(debug_assertions)]
            println!(
                "Sending update: state=\"{}\", details=\"{}\", start_time=\"{}\"",
                phases.clone().unwrap_or(String::from("None")),
                &info,
                &start_time.unwrap_or(-1)
            );

            if let Err(e) = client.set_activity(activity) {
                eprintln!("Error setting activity: {}", e);
                should_reconnect = true;
            }

            // if we encountered errors on the way we should probably reconnect...
            if should_reconnect {
                eprintln!("Encountered an error talking to Discord... trying to reconnect");
                while let Err(e) = client.reconnect().map_err(|e| e.to_string()) {
                    eprintln!("Connecting to Discord failed: {}", e);
                    eprintln!("Retrying in 5 seconds");
                    sleep(Duration::from_secs(5)).await;
                }
            }
        }

        Err(String::from("Connection to process watcher died"))
    }
}
