pub struct Patch {
    pub search: &'static str,
    pub replace: &'static str,
}

/// All patches are applied to every JS and HTML response.
/// If `search` is not found in a file the replacement is a no-op.
pub static PATCHES: &[Patch] = &[
    // Patches will be added here once we identify the relevant
    // strings in the jellyfin-web bundle.
    // Example:
    // Patch { search: "id=\"txtMediaPath\"", replace: "id=\"txtMediaPath\" hidden" },
];
