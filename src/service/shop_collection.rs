//! `ShopCollectionService`: pass-through methods generated from the linejs service.
//!
//! Path: `/ext/sapi/sapic`.

use super::ShopCollectionService;
use crate::error::Result;
use crate::thrift::Value;

// <generated pass-through>
impl ShopCollectionService<'_> {
    /// `addItemToCollection(request: AddItemToCollectionRequest)`.
    pub async fn add_item_to_collection(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("addItemToCollection", args).await
    }

    /// `createCollectionForUser(request: YN0_Ob1_A)`.
    pub async fn create_collection_for_user(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("createCollectionForUser", args).await
    }

    /// `getUserCollections(request: GetUserCollectionsRequest)`.
    pub async fn get_user_collections(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("getUserCollections", args).await
    }

    /// `isProductForCollections(request: IsProductForCollectionsRequest)`.
    pub async fn is_product_for_collections(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("isProductForCollections", args).await
    }

    /// `removeItemFromCollection(request: RemoveItemFromCollectionRequest)`.
    pub async fn remove_item_from_collection(&self, args: impl Into<Value>) -> Result<Value> {
        self.call("removeItemFromCollection", args).await
    }
}
// </generated pass-through>
