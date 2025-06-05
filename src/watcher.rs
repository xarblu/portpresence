use psutil::Pid;
use psutil::process::ProcessCollector;
use std::collections::HashMap;
use tokio::sync::mpsc::Sender;
use tokio::time::{self, Duration};

use crate::REFRESH_INTERVAL;

/// job metadata
#[derive(Clone)]
pub(crate) struct EbuildJob {
    pub(crate) category: String,
    pub(crate) package: String,
    pub(crate) version: String,
    pub(crate) phase: String,
    pub(crate) create_time: Duration,
}

/// struct for tracking ebuild processes
pub(crate) struct EmergeProcWatcher {
    /// active jobs
    /// HashMap ensures we don't capture jobs multiple times
    active: HashMap<Pid, EbuildJob>,

    /// sender for updates
    tx: Sender<Vec<EbuildJob>>,
}

impl EmergeProcWatcher {
    /// create new EmergeProcWatcher
    pub(crate) fn new(tx: Sender<Vec<EbuildJob>>) -> Self {
        Self {
            active: HashMap::new(),
            tx,
        }
    }

    /// continuesly watch processes for matches
    /// and update active table
    pub(crate) async fn start(mut self) -> ! {
        // if this fails we want the panic
        let mut collector = ProcessCollector::new().unwrap();

        let mut interval = time::interval(Duration::from_secs(REFRESH_INTERVAL));
        loop {
            interval.tick().await;
            if let Err(e) = collector.update() {
                eprintln!("Error updating processes: {}", e);
                continue;
            }

            // clear out current jobs
            self.active.clear();

            // look for running ebuild processes
            for (pid, process) in &collector.processes {
                let cmdline = match process.cmdline_vec() {
                    Ok(maybe_cmdline) => match maybe_cmdline {
                        Some(cmdline) => cmdline,
                        None => continue, // kernel thread
                    },
                    Err(_) => continue, // process died already
                };

                // now we look for any ebuild process like:
                // bash /usr/lib/portage/pypy3.11/ebuild.sh unpack
                if cmdline.len() != 3 {
                    continue;
                }

                // check if cmdline matches
                if !cmdline[1].ends_with("ebuild.sh") {
                    continue;
                }

                #[cfg(debug_assertions)]
                println!("Found ebuild process: {}", &pid);

                // gather infos by walking up the tree
                let mut current = process.clone();
                loop {
                    // go up one layer
                    current = match current.parent() {
                        Ok(ps) => match ps {
                            Some(ps) => ps,
                            None => break, // parent dead
                        },
                        Err(_) => break, // current dead
                    };

                    #[cfg(debug_assertions)]
                    println!("Parsing process {}", current.pid());

                    // cmdline_vec() doesn't help us because apparently
                    // the sandbox likes to merge multiple args...
                    let cmdline_str = match current.cmdline() {
                        Ok(maybe_cmdline) => match maybe_cmdline {
                            Some(cmdline) => cmdline,
                            None => continue, // kernel thread
                        },
                        Err(_) => continue, // process died
                    };

                    let cmdline: Vec<&str> = cmdline_str.split_ascii_whitespace().collect();

                    #[cfg(debug_assertions)]
                    println!("It has {} parts: {}", cmdline.len(), cmdline.join(", "));

                    // we want a sandbox process like (won't qu):
                    // [sys-kernel/cachyos-kernel-6.15.1] sandbox /usr/lib/portage/pypy3.11/ebuild.sh compile
                    if cmdline.len() == 4
                        && cmdline[0].starts_with("[")
                        && cmdline[0].ends_with("]")
                        && cmdline[1] == "sandbox"
                        && cmdline[2].ends_with("ebuild.sh")
                    {
                        #[cfg(debug_assertions)]
                        println!("Process {} looks correct...", current.pid());

                        let cpv = cmdline[0].trim_matches(['[', ']']);
                        let (c, pv) = cpv.split_once('/').unwrap();

                        let mut p = String::new();
                        let mut v = String::new();
                        let mut p_complete = false;
                        for part in pv.split('-') {
                            // start v on first number
                            if part.starts_with(['0', '1', '2', '3', '4', '5', '6', '7', '8', '9'])
                            {
                                p_complete = true;
                            }

                            if !p_complete {
                                if !p.is_empty() {
                                    p.push('-');
                                }
                                p.push_str(part);
                            } else {
                                if !v.is_empty() {
                                    v.push('-');
                                }
                                v.push_str(part);
                            }
                        }

                        self.active.insert(
                            current.pid(),
                            EbuildJob {
                                category: String::from(c),
                                package: p,
                                version: v,
                                phase: String::from(cmdline[3]),
                                create_time: current.create_time(),
                            },
                        );

                        break; // got all we need from this tree
                    }
                }
            }

            // if we have jobs send them
            if !self.active.is_empty() {
                self.tx
                    .send(self.active.values().cloned().collect())
                    .await
                    .unwrap();
            }
        }
    }
}
