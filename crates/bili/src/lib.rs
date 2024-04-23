#![allow(unused_imports)]
// #![allow(dead_code)]

pub mod breakpoint;
pub mod consts;
pub mod facade;

mod error;
mod impls;
mod models;

pub use error::*;
pub use models::*;

pub use impls::*;

pub trait DebugPrint {
    fn debug_print(self) -> Self;
}
impl<T> DebugPrint for T
where
    T: std::fmt::Debug,
{
    fn debug_print(self) -> Self {
        println!("{:?}", self);
        self
    }
}
