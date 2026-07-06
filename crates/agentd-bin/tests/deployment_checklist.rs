use std::path::PathBuf;

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn read_repo_file(path: &str) -> String {
    std::fs::read_to_string(repo_root().join(path)).unwrap_or_else(|err| {
        panic!("read {path}: {err}");
    })
}

fn task_assignment_gap_line(checklist: &str) -> &str {
    checklist
        .lines()
        .find(|line| line.contains("**TaskAssignment**"))
        .expect("TaskAssignment known-gap line")
}

#[test]
fn deployment_checklist_marks_p121_agent_id_gap_resolved() {
    let checklist = read_repo_file("docs/p0.9-deployment-checklist.md");
    let p121 = read_repo_file("specs/e2e/p121-production-assign-task-agent-ownership.spec.md");
    let line = task_assignment_gap_line(&checklist);

    assert!(
        p121.contains("ProductionRunHost::open_task")
            && p121.contains("TaskAssignment.agent_id")
            && p121.contains("legacy rows with a null agent id"),
        "P121 spec should document the resolved production ownership behavior"
    );
    assert!(
        line.contains("P121") && line.contains("agent_id"),
        "TaskAssignment gap line should explicitly mention P121 resolved agent_id: {line}"
    );
    assert!(
        !line.contains("agent_id`/`spec_path`/`plan_path` are populated from the spawn context"),
        "line still describes agent_id as an unresolved spawn-context gap: {line}"
    );
}

#[test]
fn deployment_checklist_keeps_spec_and_plan_path_gaps() {
    let checklist = read_repo_file("docs/p0.9-deployment-checklist.md");
    let line = task_assignment_gap_line(&checklist);

    assert!(
        line.contains("spec_path") && line.contains("plan_path"),
        "TaskAssignment gap line should keep spec_path and plan_path as remaining gaps: {line}"
    );
}
