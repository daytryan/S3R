//! Retained pointer `SetSQT82a2f3d8` / buffered `SetData82a2c828`.
use super::{ActorFaceError, BindPoseError, Command, TreeCommands};
use crate::actor_animation_construction::initialization::bind_pose::sqt_output::OwnedAnimationSqtBuffer;
impl TreeCommands {
    /// ReplaceSQT82a253b0 emits a partial SetData followed by an in-place
    /// range replacement. A fixed first operand instead writes the new slot.
    pub(crate) fn replace_external_sqt(&mut self,source:&OwnedAnimationSqtBuffer,
        offset:usize,count:usize)->Result<(),BindPoseError>{
        let source=source.try_subrange(0..count)?;
        self.set_external_sqt(&source)?;
        self.external_set_count+=1;
        if self.external_set_count>self.owner.lock().map_err(|_|ActorFaceError::Lock)?.max_set_replace_sqts{
            return Err(BindPoseError::UnsupportedSetup);
        }
        let offset=u8::try_from(offset).map_err(|_|BindPoseError::InvalidHierarchy)?;
        let count=u8::try_from(count).map_err(|_|BindPoseError::InvalidHierarchy)?;
        self.binary(false,|first,second,output|Command::ReplaceSqt{first,second,output,offset,count})
    }
    pub(crate) fn accepts_external_sqt_size(&self,slot:u16,length:usize,bones:usize)->bool{
        length==bones||self.operations.iter().any(|op|matches!(op,
            Command::ReplaceSqt{second,count,..} if *second==slot&&usize::from(*count)==length))
    }
    /// Retains the source allocation. `ACSsetData_job82a24000` samples its
    /// contents at execution, after command admission and scheduling.
    pub(crate) fn set_external_sqt(
        &mut self,
        source: &OwnedAnimationSqtBuffer,
    ) -> Result<(), BindPoseError> {
        {
            let owner = self.owner.lock().map_err(|_| ActorFaceError::Lock)?;
            if owner.in_commands == 0 || owner.in_command_set == 0 {
                return Err(BindPoseError::AlreadyInCommands);
            }
        }
        let sqt = self.allocate_dynamic()?; // native c84c, before usage increment.
        self.external_set_count += 1;
        let limit = self
            .owner
            .lock()
            .map_err(|_| ActorFaceError::Lock)?
            .max_set_replace_sqts;
        if self.external_set_count > limit {
            return Err(BindPoseError::UnsupportedSetup);
        }
        self.external_sqts.push((sqt, source.clone()));
        self.owner
            .lock()
            .map_err(|_| ActorFaceError::Lock)?
            .commands
            .push(Command::SetExternalSqt { sqt });
        // SetData82a2c828 emits opcode12 without bit0 and a pid -1 graph node:
        // a dynamic root that later reuse compaction must remap and copy.
        self.produced(&[], sqt);
        self.stack.push(sqt);
        Ok(())
    }
}
