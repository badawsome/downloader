include!(concat!(env!("OUT_DIR"), "/base.rs"));

pub mod facade {
    pub mod items {
        include!(concat!(env!("OUT_DIR"), "/facade.items.rs"));
    }
}

use facade::items;

pub fn create_large_shirt(color: String) -> items::Shirt {
    let mut shirt = items::Shirt::default();
    shirt.color = color;
    shirt.set_size(items::shirt::Size::Large);
    shirt
}