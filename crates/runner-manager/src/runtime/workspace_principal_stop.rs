use runner_protocol::RunnerOwner;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, Weak},
};
use tokio::sync::RwLock;

#[derive(Default)]
pub(super) struct OwnerFences(Mutex<HashMap<RunnerOwner, Weak<RwLock<()>>>>);

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn fences_serialize_same_owner_but_not_other_owners_and_reap_keys() {
        let fences = OwnerFences::default();
        let first = RunnerOwner::InternalUser { user_id: 1 };
        let second = RunnerOwner::InternalUser { user_id: 2 };
        let a = fences.for_owner(&first);
        let b = fences.for_owner(&first);
        let c = fences.for_owner(&second);
        let guard = a.write().await;
        assert!(
            tokio::time::timeout(Duration::from_millis(10), b.read())
                .await
                .is_err()
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(10), c.read())
                .await
                .is_ok()
        );
        drop(guard);
        drop(a);
        drop(b);
        drop(c);
        let _new = fences.for_owner(&second);
        assert_eq!(fences.0.lock().unwrap().len(), 1);
    }
}

impl OwnerFences {
    pub(super) fn for_owner(&self, owner: &RunnerOwner) -> Arc<RwLock<()>> {
        let mut fences = self.0.lock().expect("owner fences poisoned");
        fences.retain(|_, fence| fence.strong_count() != 0);
        if let Some(fence) = fences.get(owner).and_then(Weak::upgrade) {
            return fence;
        }
        let fence = Arc::new(RwLock::new(()));
        fences.insert(owner.clone(), Arc::downgrade(&fence));
        fence
    }
}
