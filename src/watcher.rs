use psutil::process::ProcessCollector;
use tokio::time::{self, Duration};
use futures::lock::Mutex;

// FIXME String is placeholder for now
type EbuildJob = String;

/// struct for tracking ebuild processes
pub(crate) struct EbuildProcWatcher {
    /// active ebuild processes
    active: Mutex<Vec<EbuildJob>>,
}

impl EbuildProcWatcher {
    /// continuesly watch processes for matches
    /// and update active table
    pub(crate) async fn new(&self) -> ! {
        // if this fails we want the panic
        let mut collector = ProcessCollector::new().unwrap();
        
        let mut interval = time::interval(Duration::from_secs(5));
        loop {
            interval.tick().await;

            // TODO matching logic
        }
    }

    /// return copy of currently running jobs
    pub(crate) async fn active(&self) -> Vec<EbuildJob> {
        self.active.lock().await.clone()
    }
}
