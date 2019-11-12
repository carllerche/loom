use crate::rt::VersionVecSlice;
use bumpalo::Bump;

#[derive(Debug)]
pub(crate) struct Access<'bump> {
    path_id: usize,
    dpor_vv: VersionVecSlice<'bump>,
}

impl<'bump> Access<'bump> {
    pub(crate) fn new(
        path_id: usize,
        version: &VersionVecSlice<'_>,
        bump: &'bump Bump,
    ) -> Access<'bump> {
        Access {
            path_id,
            dpor_vv: VersionVecSlice::clone_in(version, bump),
        }
    }

    pub(crate) fn set(&mut self, path_id: usize, version: &VersionVecSlice<'_>) {
        self.path_id = path_id;
        self.dpor_vv.set(version);
    }

    pub(crate) fn set_or_create_in(
        access: &mut Option<Self>,
        path_id: usize,
        version: &VersionVecSlice<'_>,
        bump: &'bump Bump,
    ) {
        if let Some(access) = access.as_mut() {
            access.set(path_id, version);
        } else {
            *access = Some(Access::new(path_id, version, bump));
        }
    }

    /// Location in the path
    pub(crate) fn path_id(&self) -> usize {
        self.path_id
    }

    pub(crate) fn version(&self) -> &VersionVecSlice<'_> {
        &self.dpor_vv
    }

    pub(crate) fn happens_before(&self, version: &VersionVecSlice<'_>) -> bool {
        self.dpor_vv <= *version
    }
}
