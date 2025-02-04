use super::{Context, FPUContext, Task};
use crate::{
    allocator::malloc,
    collections::{list::RawNode, queue::RawQueue},
};
use alloc::collections::BTreeMap;
use core::ptr::NonNull;
use log::debug;

const TASKPOOL_SIZE: usize = 1024;
pub struct TaskManager {
    empty_queue: RawQueue<Task>,
    task_map: BTreeMap<u64, NonNull<Task>>,
    max_count: usize,
    use_count: usize,
    alloc_count: usize,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            empty_queue: RawQueue::new(),
            task_map: BTreeMap::new(),
            use_count: 0,
            alloc_count: 0,
            max_count: TASKPOOL_SIZE,
        }
    }

    pub fn allocate(&mut self) -> Result<&'static mut Task, ()> {
        const TASK_SIZE: usize = size_of::<Task>();
        if self.use_count >= TASKPOOL_SIZE {
            return Err(());
        }

        let task = match self.empty_queue.pop() {
            Some(task) => task,
            None => {
                let task_ptr = malloc(size_of::<Task>(), 8).cast::<Task>();
                // debug!("task_ptr = {task_ptr:?}, size = {TASK_SIZE:#X}");
                if let Some(mut task) = NonNull::new(task_ptr) {
                    unsafe { task.as_mut() }
                } else {
                    return Err(());
                }
            }
        };
        task.set_id(self.alloc_count as u64);
        self.task_map.insert(task.id(), NonNull::new(task).unwrap());

        self.alloc_count = self.alloc_count.wrapping_add(1);
        self.use_count += 1;
        Ok(task)
    }

    pub fn free(&mut self, task: &mut Task) {
        self.task_map.remove(&task.id());

        task.set_parent(None);
        task.set_child(None);
        task.set_sibling(None);
        task.set_prev(None);
        task.set_next(None);
        *task.context() = Context::empty();
        *task.fpu_context() = FPUContext::new();

        self.empty_queue.push(task);
        self.use_count -= 1;
    }

    pub fn get(&mut self, id: u64) -> Option<&'static mut Task> {
        // let task_ptr = self.task_map.get(&id);
        // debug!("task_ptr = {task_ptr:?}");
        // Some(unsafe { self.task_map.get_mut(&id)?.as_mut() })
        self.task_map.iter_mut().find_map(|(&task_id, task)| {
            if task_id == id {
                unsafe { Some(task.as_mut()) }
            } else {
                None
            }
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = &Task> {
        self.task_map
            .iter()
            .map(|(id, task)| unsafe { task.as_ref() })
    }
}
