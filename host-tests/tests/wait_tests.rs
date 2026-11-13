use hosttest::wait::{
    decide, ChildSnapshot, ChildState, WaitDecision, WaitTarget,
};

fn child(pid: u32, parent: u32, state: ChildState, code: Option<i32>) -> ChildSnapshot {
    ChildSnapshot {
        pid,
        parent: Some(parent),
        state,
        exit_code: code,
    }
}

#[test]
fn no_child_is_reported() {
    assert_eq!(
        decide(4, WaitTarget::Any, &[]),
        WaitDecision::NoChild
    );
    assert_eq!(
        decide(
            4,
            WaitTarget::Pid(9),
            &[child(5, 4, ChildState::Zombie, Some(0))]
        ),
        WaitDecision::NoChild
    );
}

#[test]
fn running_child_blocks_and_zombie_is_reaped() {
    assert_eq!(
        decide(
            4,
            WaitTarget::Pid(5),
            &[child(5, 4, ChildState::Running, None)]
        ),
        WaitDecision::Block
    );
    assert_eq!(
        decide(
            4,
            WaitTarget::Pid(5),
            &[child(5, 4, ChildState::Zombie, Some(7))]
        ),
        WaitDecision::Reap(5, 7)
    );
}

#[test]
fn any_child_selects_the_first_zombie() {
    let children = [
        child(5, 4, ChildState::Running, None),
        child(6, 4, ChildState::Zombie, Some(3)),
        child(7, 4, ChildState::Zombie, Some(9)),
    ];
    assert_eq!(
        decide(4, WaitTarget::Any, &children),
        WaitDecision::Reap(6, 3)
    );
    assert_eq!(
        decide(4, WaitTarget::Pid(7), &children),
        WaitDecision::Reap(7, 9)
    );
    assert_eq!(
        decide(4, WaitTarget::Pid(5), &children),
        WaitDecision::Block
    );
}

#[test]
fn ignores_other_parents() {
    let children = [
        child(5, 8, ChildState::Zombie, Some(1)),
        child(6, 4, ChildState::Running, None),
    ];
    assert_eq!(
        decide(4, WaitTarget::Any, &children),
        WaitDecision::Block
    );
    assert!(WaitTarget::Any.matches(99));
    assert!(WaitTarget::Pid(7).matches(7));
    assert!(!WaitTarget::Pid(7).matches(8));
}
