//! A deliberately small task tracker for Beacon's opt-in Sample Project.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Open,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Task {
    title: String,
    state: TaskState,
}

#[derive(Debug, Default)]
pub struct TaskTracker {
    tasks: Vec<Task>,
}

impl TaskTracker {
    pub fn add(&mut self, title: impl Into<String>) -> Result<(), &'static str> {
        let title = title.into();
        if title.trim().is_empty() {
            return Err("task title cannot be empty");
        }
        self.tasks.push(Task {
            title,
            state: TaskState::Open,
        });
        Ok(())
    }

    pub fn complete(&mut self, index: usize) -> Result<(), &'static str> {
        let task = self.tasks.get_mut(index).ok_or("task was not found")?;
        task.state = TaskState::Done;
        Ok(())
    }

    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }
}

impl Task {
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn state(&self) -> TaskState {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::{TaskState, TaskTracker};

    #[test]
    fn task_moves_from_open_to_done() {
        let mut tracker = TaskTracker::default();
        tracker.add("Verify the proving loop").unwrap();

        assert_eq!(tracker.tasks()[0].state(), TaskState::Open);
        tracker.complete(0).unwrap();

        assert_eq!(tracker.tasks()[0].title(), "Verify the proving loop");
        assert_eq!(tracker.tasks()[0].state(), TaskState::Done);
    }

    #[test]
    fn empty_titles_and_unknown_tasks_are_refused() {
        let mut tracker = TaskTracker::default();

        assert_eq!(tracker.add("   "), Err("task title cannot be empty"));
        assert_eq!(tracker.complete(0), Err("task was not found"));
    }
}
