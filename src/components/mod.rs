pub mod hero;
pub use hero::*;

pub mod video;
pub use video::*;
//mod navbar;
//pub use navbar::Navbar;

mod sidebar;
pub use sidebar::*;

mod card;
pub use card::*;

mod button;
pub use button::*;

pub mod media_row;
pub use media_row::*;

pub mod bottom_navbar;
pub use bottom_navbar::*;

//#[cfg(target_arch = "wasm32")]
//pub mod sheet;
//#[cfg(target_arch = "wasm32")]
//pub use sheet::*;

//#[cfg(not(target_arch = "wasm32"))]
pub mod sheet_eval;
//#[cfg(not(target_arch = "wasm32"))]
pub use sheet_eval::*;

pub mod image;
pub use image::*;

pub mod icon;
pub use icon::*;

pub mod progress;
pub use progress::*;

pub mod list;
pub use list::*;

pub mod switch;
pub use switch::*;

//#[cfg(target_arch = "wasm32")]
//pub mod virtual_list;
//#[cfg(target_arch = "wasm32")]
//pub use virtual_list::*;

// #[cfg(not(target_arch = "wasm32"))]
//pub mod paginated_list_native;
// #[cfg(not(target_arch = "wasm32"))]
//pub use paginated_list_native::*;
// #[cfg(not(target_arch = "wasm32"))]
//pub use paginated_list_native::PaginatedList as CarouselList;

//#[cfg(not(target_arch = "wasm32"))]
pub mod paginated_list_eval;
//#[cfg(not(target_arch = "wasm32"))]
pub use paginated_list_eval::*;
//#[cfg(not(target_arch = "wasm32"))]
pub use paginated_list_eval::PaginatedList as CarouselList;
