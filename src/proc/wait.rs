#![allow(dead_code)]
use super::pid::Pid;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitTarget {
    Any,
    Pid(Pid),
}

impl WaitTarget {
    pub fn matches(self, pid: Pid) -> bool {
        match self {
            WaitTarget::Any => true,
            WaitTarget::Pid(target) => target == pid,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChildState {
    Running,
    Zombie,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChildSnapshot {
    pub pid: Pid,
    pub parent: Option<Pid>,
    pub state: ChildState,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitDecision {
    Reap(Pid, i32),
    Block,
    NoChild,
}

pub fn decide(parent: Pid, target: WaitTarget, children: &[ChildSnapshot]) -> WaitDecision {
    let mut matched = false;
    for child in children {
        if child.parent != Some(parent) || !target.matches(child.pid) {
            continue;
        }
        matched = true;
        if child.state == ChildState::Zombie {
            return WaitDecision::Reap(child.pid, child.exit_code.unwrap_or(0));
        }
    }
    if matched {
        WaitDecision::Block
    } else {
        WaitDecision::NoChild
    }
}
