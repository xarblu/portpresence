mod watcher;

use tokio::sync::mpsc;
use tokio::task;

use crate::watcher::EbuildJob;

/// Discord API client ID
const CLIENT_ID: &str = "CHANGEME";

// refresh interval in seconds
const REFRESH_INTERVAL: u64 = 5;

#[tokio::main]
async fn main() {
    let (tx, mut rx) = mpsc::channel::<Vec<EbuildJob>>(1);

    let watcher = watcher::EmergeProcWatcher::new(tx);
    task::spawn(watcher.start());

    while let Some(jobs) = rx.recv().await {
        #[cfg(debug_assertions)]
        println!("Got Jobs");
        for job in jobs {
            #[cfg(debug_assertions)]
            println!(
                "{}, {}, {}, {}, {}",
                job.category,
                job.package,
                job.version,
                job.phase,
                job.create_time.as_secs(),
            )
        }
    }
}
