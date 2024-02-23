use subsquid_messages::RangeSet;
use crate::types::state::ChunkRef;

pub struct DatasetTracker {

}


impl DatasetTracker {
    pub fn set_desired_state(&self, desired: &RangeSet) {

    }

    pub async fn get_state_update(&self) -> StateUpdate {
        todo!()
    }
}


pub struct StateUpdate {
    new: Vec<ChunkRef>,
    deleted: Vec<ChunkRef>
}