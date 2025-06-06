mod rpchandler;
mod watcher;

use tokio::sync::mpsc;
use tokio::task::JoinSet;

use crate::rpchandler::RPCHandler;
use crate::watcher::{ActiveJobs, EbuildProcWatcher};

/// Discord API client ID
const CLIENT_ID: &str = "1367276666665041960";

// process refresh interval in seconds
const REFRESH_INTERVAL: u64 = 5;

#[tokio::main]
async fn main() {
    let (tx, rx) = mpsc::channel::<ActiveJobs>(1);

    let mut tasks = JoinSet::new();

    let watcher = EbuildProcWatcher::new(tx);
    tasks.spawn(watcher.start());

    let rpchandler = RPCHandler::new(rx);
    tasks.spawn(rpchandler.start());

    tasks.join_all().await;
}
