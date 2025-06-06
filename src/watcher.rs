use procfs::{Current, Uptime};
use psutil::Pid;
use psutil::process::{Process, ProcessCollector};
use std::collections::{BTreeMap, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::mpsc::Sender;
use tokio::time::{self, Duration};

use crate::REFRESH_INTERVAL;

pub(crate) type ActiveJobs = HashMap<Pid, HashMap<Pid, EbuildJob>>;

/// job metadata
#[derive(Clone, PartialEq)]
pub(crate) struct EbuildJob {
    /// ebuild category
    pub(crate) category: String,

    /// ebuild package
    pub(crate) package: String,

    /// ebuild version
    pub(crate) version: String,

    /// ebuild phase
    pub(crate) phase: String,

    /// job process creation time in unix epoch duration
    /// because each phase start a new sandbox process
    /// this will reset with each phase
    /// TODO: maybe walk further up the proc tree and match `emerge` process
    pub(crate) create_time: Duration,
}

/// struct for tracking ebuild processes
pub(crate) struct EbuildProcWatcher {
    /// active jobs as: {"emerge master pid": {"ebuild job pid": {job...}}}
    /// HashMap ensures we don't capture jobs multiple times
    active: ActiveJobs,

    /// sender for updates
    tx: Sender<ActiveJobs>,
}

impl EbuildProcWatcher {
    /// create new EmergeProcWatcher
    pub(crate) fn new(tx: Sender<ActiveJobs>) -> Self {
        Self {
            active: HashMap::new(),
            tx,
        }
    }

    /// continuesly watch processes for matches
    /// and update active table
    pub(crate) async fn start(mut self) -> Result<(), String> {
        // if this fails we want the panic
        let mut collector = ProcessCollector::new().unwrap();

        let mut interval = time::interval(Duration::from_secs(REFRESH_INTERVAL));
        loop {
            interval.tick().await;
            if let Err(e) = collector.update() {
                eprintln!("Error updating processes: {}", e);
                continue;
            }

            // track if we actually changed something
            let mut changed = false;

            // remove finished jobs
            let masters: Vec<Pid> = self.active.keys().cloned().collect();
            for master in masters {
                // first check if we can remove an entire subtree
                if !&collector.processes.contains_key(&master)
                    && self.active.remove(&master).is_some()
                {
                    changed = true;
                    continue;
                }

                // the check jobs under master
                let jobs: Vec<Pid> = self.active.get(&master).unwrap().keys().cloned().collect();
                for job in jobs {
                    if !&collector.processes.contains_key(&job)
                        && self.active.get_mut(&master).unwrap().remove(&job).is_some()
                    {
                        changed = true;
                        continue;
                    }
                }
            }

            // grab all running emerge processes and make sure they
            // exist in our tree
            let emerge_procs = get_emerge_procs(&collector.processes);
            for process in emerge_procs {
                if self.active.contains_key(&process.pid()) {
                    continue;
                }
                self.active.insert(process.pid(), HashMap::new());
                changed = true;
            }

            // look for running ebuild processes
            for (_pid, process) in &collector.processes {
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
                println!("Found ebuild process: {}", &_pid);

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
                    println!("Parsing parent process {}", current.pid());

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

                    // we want a sandbox process like:
                    // [sys-kernel/cachyos-kernel-6.15.1] sandbox /usr/lib/portage/pypy3.11/ebuild.sh compile
                    if cmdline.len() == 4
                        && cmdline[0].starts_with("[")
                        && cmdline[0].ends_with("]")
                        && cmdline[1] == "sandbox"
                        && cmdline[2].ends_with("ebuild.sh")
                    {
                        #[cfg(debug_assertions)]
                        println!("Process {} looks correct...", current.pid());

                        // try to find master process, if that doesn't exist drop this job
                        // this means we won't match manual `ebuild` invocations
                        let master = match get_managing_emerge_proc(&current) {
                            Some(ps) => ps,
                            None => break,
                        };

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

                        let new = EbuildJob {
                            category: String::from(c),
                            package: p,
                            version: v,
                            phase: String::from(cmdline[3]),
                            create_time: proc_time_to_unix_time(current.create_time()),
                        };

                        match self.active.get(&master.pid()) {
                            // full tree not present
                            None => {
                                self.active.insert(master.pid(), HashMap::new());
                                self.active
                                    .get_mut(&master.pid())
                                    .unwrap()
                                    .insert(current.pid(), new);
                                changed = true;
                            }
                            Some(map) => match map.get(&current.pid()) {
                                // job not present in tree
                                None => {
                                    self.active
                                        .get_mut(&master.pid())
                                        .unwrap()
                                        .insert(current.pid(), new);
                                    changed = true;
                                }
                                Some(old) => {
                                    // if jobs are equal we don't want an update
                                    if new == old.clone() {
                                        break;
                                    }
                                    // job present and new one different
                                    self.active
                                        .get_mut(&master.pid())
                                        .unwrap()
                                        .insert(current.pid(), new);
                                    changed = true;
                                }
                            },
                        }

                        break; // got all we need from this tree
                    }
                }
            }

            // send the job list if changed
            match changed {
                false => {
                    #[cfg(debug_assertions)]
                    println!("Job list unchanged ({} items)", self.active.len());
                }
                true => {
                    #[cfg(debug_assertions)]
                    println!("Job list updated ({} items)", self.active.len());

                    if self.tx.send(self.active.clone()).await.is_err() {
                        return Err(String::from("Connection to RPC handler died"));
                    }
                }
            }
        }
    }
}

/// convert process creation time to unix time
fn proc_time_to_unix_time(proc_time: Duration) -> Duration {
    let uptime = Uptime::current().unwrap().uptime_duration();
    let currtime = SystemTime::now().duration_since(UNIX_EPOCH).unwrap();
    currtime - uptime + proc_time
}

/// parse a process list for emerge processes
fn get_emerge_procs(processes: &BTreeMap<Pid, Process>) -> Vec<Process> {
    #[cfg(debug_assertions)]
    println!("Looking for emerge processes");

    let mut emerge_procs = Vec::new();
    for process in processes.values() {
        let cmdline_str = match process.cmdline() {
            Ok(maybe_cmdline) => match maybe_cmdline {
                Some(cmdline) => cmdline,
                None => continue, // kernel thread
            },
            Err(_) => continue, // process died already
        };

        let cmdline: Vec<&str> = cmdline_str.split_ascii_whitespace().collect();

        // now we look for any emerge process like:
        // /usr/bin/pypy3.11 /usr/lib/python-exec/pypy3.11/emerge args...
        if cmdline.len() < 2 {
            continue;
        }

        // check if cmdline matches
        // leading "/" makes this not match e.g. sudo emerge
        if cmdline[1].ends_with("/emerge") {
            #[cfg(debug_assertions)]
            println!("Found emerge process {}: {}", process.pid(), cmdline_str);

            emerge_procs.push(process.clone());
        }
    }
    emerge_procs
}

/// get managing emerge process of process like
/// /usr/bin/pypy3.11 /usr/lib/python-exec/pypy3.11/emerge args...
/// we will match the first one in case of e.g. `sudo emerge ..args`
fn get_managing_emerge_proc(process: &Process) -> Option<Process> {
    #[cfg(debug_assertions)]
    println!("Looking for managing emerge for {}", process.pid());

    let mut current = process.clone();
    loop {
        // go up one layer
        current = match current.parent() {
            Ok(ps) => ps?,         // returns None if parent dead
            Err(_) => return None, // current dead
        };

        #[cfg(debug_assertions)]
        println!("Parsing parent process {}", current.pid());

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

        if cmdline.len() < 2 {
            continue;
        }

        if cmdline[1].ends_with("/emerge") {
            #[cfg(debug_assertions)]
            println!("Found emerge process {}: {}", current.pid(), cmdline_str);

            return Some(current);
        }
    }
}
