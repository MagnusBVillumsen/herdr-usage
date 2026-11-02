use std::process::Command;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::shell;

#[derive(Clone)]
pub struct Machine {
    pub label: String,
    /// The saved Herdr machine's SSH target; `None` for this machine.
    pub target: Option<String>,
}

pub struct Load {
    pub cpu: f64,
    pub mem_used: u64,
    pub mem_total: u64,
    pub load: f64,
    pub cores: u32,
}

#[derive(Default)]
pub struct Agents {
    pub working: usize,
    pub blocked: usize,
    pub idle: usize,
}

pub struct Status {
    pub load: Result<Load>,
    pub agents: Option<Agents>,
}

/// Prints `cpu <percent>`, `mem <used> <total>` and `load <1m> <cores>` on
/// Linux and macOS.
const PROBE: &str = r#"
if [ -r /proc/stat ]; then
  c() { awk '/^cpu /{t=0; for(i=2;i<=NF;i++) t+=$i; print t, $5+$6}' /proc/stat; }
  a=$(c); sleep 0.5; b=$(c)
  echo "$a $b" | awk '{dt=$3-$1; di=$4-$2; printf "cpu %.1f\n", (dt > 0 ? 100*(dt-di)/dt : 0)}'
  awk '/^MemTotal:/{t=$2} /^MemAvailable:/{a=$2} END{printf "mem %d %d\n", (t-a)*1024, t*1024}' /proc/meminfo
  echo "load $(cut -d' ' -f1 /proc/loadavg) $(getconf _NPROCESSORS_ONLN)"
else
  top -l 2 -n 0 -s 1 | awk '/CPU usage/{u=$3+$5} END{printf "cpu %.1f\n", u}'
  vm_stat | awk -v total="$(sysctl -n hw.memsize)" '/page size of/{ps=$8} /Pages active/{a=$3} /Pages wired/{w=$4} /occupied by compressor/{c=$5} END{printf "mem %d %d\n", (a+w+c)*ps, total}'
  echo "load $(sysctl -n vm.loadavg | awk '{print $2}') $(sysctl -n hw.ncpu)"
fi
"#;

fn herdr() -> Command {
    Command::new(std::env::var("HERDR_BIN_PATH").unwrap_or_else(|_| "herdr".into()))
}

/// This machine first, then every enabled saved Herdr machine.
pub fn list() -> Vec<Machine> {
    let host = Command::new("uname")
        .arg("-n")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|h| !h.is_empty())
        .unwrap_or_else(|| "local".into());
    let host = host.split('.').next().unwrap_or(&host).to_owned();
    let mut machines = vec![Machine {
        label: host,
        target: None,
    }];
    let saved = herdr()
        .args(["machine", "list", "--json"])
        .output()
        .ok()
        .and_then(|o| serde_json::from_slice::<Vec<Value>>(&o.stdout).ok())
        .unwrap_or_default();
    for m in saved.iter().filter(|m| m["enabled"] != false) {
        if let (Some(label), Some(target)) = (m["label"].as_str(), m["target"].as_str()) {
            machines.push(Machine {
                label: label.into(),
                target: Some(target.into()),
            });
        }
    }
    machines
}

fn parse_load(out: &str) -> Result<Load> {
    let mut load = Load {
        cpu: 0.0,
        mem_used: 0,
        mem_total: 0,
        load: 0.0,
        cores: 1,
    };
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        match f.as_slice() {
            ["cpu", p] => load.cpu = p.parse().context("cpu")?,
            ["mem", u, t] => {
                load.mem_used = u.parse().context("mem")?;
                load.mem_total = t.parse().context("mem")?;
            }
            ["load", l, c] => {
                load.load = l.parse().context("load")?;
                load.cores = c.parse().context("cores")?;
            }
            _ => {}
        }
    }
    if load.mem_total == 0 {
        bail!("no readings");
    }
    Ok(load)
}

fn agents(m: &Machine) -> Option<Agents> {
    let mut cmd = herdr();
    if m.target.is_some() {
        cmd.args(["--machine", &m.label]);
    }
    let out = cmd.args(["agent", "list"]).output().ok()?;
    let v: Value = serde_json::from_slice(&out.stdout).ok()?;
    let mut a = Agents::default();
    for agent in v["result"]["agents"].as_array()? {
        match agent["agent_status"].as_str() {
            Some("working") => a.working += 1,
            Some("blocked") => a.blocked += 1,
            _ => a.idle += 1,
        }
    }
    Some(a)
}

pub fn probe(m: &Machine) -> Status {
    let load = shell::run(m.target.as_deref(), PROBE, &[]).and_then(|out| parse_load(&out));
    let agents = load.is_ok().then(|| agents(m)).flatten();
    Status { load, agents }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_runs_here() {
        let l = parse_load(&shell::run(None, PROBE, &[]).unwrap()).unwrap();
        assert!(l.mem_used > 0 && l.mem_used < l.mem_total);
        assert!((0.0..=100.0).contains(&l.cpu) && l.cores > 0);
    }
}
