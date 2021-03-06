// Copyright 2018 Kodebox, Inc.
// This file is part of CodeChain.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as
// published by the Free Software Foundation, either version 3 of the
// License, or (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use ckey::{Address, Public};
use primitives::{H256, U256};
use trie::Result as TrieResult;

use super::{Asset, AssetAddress, AssetScheme, AssetSchemeAddress};

pub trait TopStateInfo {
    /// Get the nonce of account `a`.
    fn nonce(&self, a: &Address) -> TrieResult<U256>;

    /// Get the balance of account `a`.
    fn balance(&self, a: &Address) -> TrieResult<U256>;

    /// Get the regular key of account `a`.
    fn regular_key(&self, a: &Address) -> TrieResult<Option<Public>>;

    fn number_of_shards(&self) -> TrieResult<u32>;

    fn shard_root(&self, shard_id: u32) -> TrieResult<Option<H256>>;

    /// Get the asset scheme.
    fn asset_scheme(&self, shard_id: u32, a: &AssetSchemeAddress) -> TrieResult<Option<AssetScheme>>;
    /// Get the asset.
    fn asset(&self, shard_id: u32, a: &AssetAddress) -> TrieResult<Option<Asset>>;
}

pub trait ShardStateInfo {
    fn root(&self) -> &H256;
    /// Get the asset scheme.
    fn asset_scheme(&self, a: &AssetSchemeAddress) -> TrieResult<Option<AssetScheme>>;
    /// Get the asset.
    fn asset(&self, a: &AssetAddress) -> TrieResult<Option<Asset>>;
}
