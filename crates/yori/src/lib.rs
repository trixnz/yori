//! Display mapping and geometry for the native yori editor.

pub mod display;
pub mod document_info;
pub mod geometry;
pub mod navigation;
/// Native Perforce review provider.
pub use yori_p4 as p4;
pub mod scrollbar;
pub mod vim;
