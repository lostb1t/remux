use crate::media::Media;

use super::{Endpoint, RestClient};
use anyhow::Result;

pub trait Pageable
where
    Self: Endpoint,
{
    //fn clone(self) -> Self where Self: Sized {
    //  self
    //}
    //type Item;

    fn set_page(&mut self, page: u32) -> &mut Self;
    // fn get_page(&self) -> u32;

    // fn bump_page(&mut self) -> &mut Self where Self: Sized {
    // 	let page = self.get_page();
    // 	self.set_page(page + 1)
    // }

    //async fn paged_media(&self, client: &RestClient) -> Result<Vec<Media>>;

    //async fn next(mut self, client: RestClient) -> Option<Self::Item> {
    //      loop {
    //        self.bump_page();
    //      }
    //  }
}

// impl<E> Pageable for &E
// where
//     E: Pageable,
// {
//     // fn use_keyset_pagination(&self) -> bool {
//     //     (*self).use_keyset_pagination()
//     // }
// }
