//! Agent identity / configuration registry.

use crate::error::AgentError;
use crate::profile::Profile;

#[architect::rpc]
pub trait Profiles {
    fn upsert_profile(&self, profile: Profile) -> Result<Profile, AgentError>;
    fn remove_profile(&self, profile_id: &str) -> Result<(), AgentError>;
    fn list_profiles(&self) -> Result<Vec<Profile>, AgentError>;
    fn read_profile(&self, profile_id: &str) -> Result<Profile, AgentError>;
}
