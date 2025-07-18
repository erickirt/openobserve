// Copyright 2025 OpenObserve Inc.
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Affero General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Affero General Public License for more details.
//
// You should have received a copy of the GNU Affero General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.

use config::meta::organization::OrganizationType;
#[cfg(feature = "cloud")]
use config::utils::time::day_micros;
use sea_orm::{
    ColumnTrait, ConnectionTrait, EntityTrait, FromQueryResult, Order, PaginatorTrait, QueryFilter,
    QueryOrder, QuerySelect, Schema, Set, entity::prelude::Expr,
};

use super::{
    entity::organizations::{ActiveModel, Column, Entity, Model},
    get_lock,
};
use crate::{
    db::{ORM_CLIENT, connect_to_orm},
    errors::{self, DbError, Error},
};

#[derive(Debug)]
pub struct OrganizationRecord {
    pub identifier: String,
    pub org_name: String,
    pub org_type: OrganizationType,
    pub created_at: i64,
    pub updated_at: i64,
    #[cfg(feature = "cloud")]
    pub trial_ends_at: i64,
}

impl OrganizationRecord {
    pub fn new(identifier: &str, org_name: &str, org_type: OrganizationType) -> Self {
        let now = chrono::Utc::now().timestamp_micros();
        Self {
            identifier: identifier.to_string(),
            org_name: org_name.to_string(),
            org_type,
            created_at: now,
            updated_at: now,
            #[cfg(feature = "cloud")]
            trial_ends_at: now + day_micros(14),
        }
    }
}

impl From<Model> for OrganizationRecord {
    fn from(model: Model) -> Self {
        Self {
            identifier: model.identifier,
            org_name: model.org_name,
            org_type: model.org_type.into(),
            created_at: model.created_at,
            updated_at: model.updated_at,
            #[cfg(feature = "cloud")]
            trial_ends_at: model.trial_ends_at,
        }
    }
}

#[derive(FromQueryResult, Debug)]
pub struct OrgId {
    pub identifier: String,
}

pub async fn create_table() -> Result<(), errors::Error> {
    let client = ORM_CLIENT.get_or_init(connect_to_orm).await;
    let builder = client.get_database_backend();

    let schema = Schema::new(builder);
    let create_table_stmt = schema
        .create_table_from_entity(Entity)
        .if_not_exists()
        .take();

    client
        .execute(builder.build(&create_table_stmt))
        .await
        .map_err(|e| Error::DbError(DbError::SeaORMError(e.to_string())))?;

    Ok(())
}

pub async fn add(
    org_id: &str,
    org_name: &str,
    org_type: OrganizationType,
) -> Result<(), errors::Error> {
    let now = chrono::Utc::now().timestamp_micros();
    let record = ActiveModel {
        identifier: Set(org_id.to_string()),
        org_name: Set(org_name.to_string()),
        org_type: Set(org_type.into()),
        created_at: Set(now),
        updated_at: Set(now),
        #[cfg(feature = "cloud")]
        trial_ends_at: Set(now + day_micros(15)),
    };

    // make sure only one client is writing to the database(only for sqlite)
    let _lock = get_lock().await;

    let client = ORM_CLIENT.get_or_init(connect_to_orm).await;
    Entity::insert(record)
        .exec(client)
        .await
        .map_err(|e| Error::DbError(DbError::SeaORMError(e.to_string())))?;

    Ok(())
}

#[cfg(feature = "cloud")]
pub async fn set_trial_period_end(org_id: &str, new_end: i64) -> Result<(), errors::Error> {
    // make sure only one client is writing to the database(only for sqlite)
    let _lock = get_lock().await;

    let client = ORM_CLIENT.get_or_init(connect_to_orm).await;
    Entity::update_many()
        .col_expr(Column::TrialEndsAt, Expr::value(new_end))
        .col_expr(
            Column::UpdatedAt,
            Expr::value(chrono::Utc::now().timestamp_micros()),
        )
        .filter(Column::Identifier.eq(org_id))
        .exec(client)
        .await
        .map_err(|e| Error::DbError(DbError::SeaORMError(e.to_string())))?;

    Ok(())
}

pub async fn rename(org_id: &str, new_name: &str) -> Result<(), errors::Error> {
    // make sure only one client is writing to the database(only for sqlite)
    let _lock = get_lock().await;

    let client = ORM_CLIENT.get_or_init(connect_to_orm).await;
    Entity::update_many()
        .col_expr(Column::OrgName, Expr::value(new_name.to_string()))
        .col_expr(
            Column::UpdatedAt,
            Expr::value(chrono::Utc::now().timestamp_micros()),
        )
        .filter(Column::Identifier.eq(org_id))
        .exec(client)
        .await
        .map_err(|e| Error::DbError(DbError::SeaORMError(e.to_string())))?;

    Ok(())
}

pub async fn remove(org_id: &str) -> Result<(), errors::Error> {
    // make sure only one client is writing to the database(only for sqlite)
    let _lock = get_lock().await;

    let client = ORM_CLIENT.get_or_init(connect_to_orm).await;
    Entity::delete_many()
        .filter(Column::Identifier.eq(org_id))
        .exec(client)
        .await
        .map_err(|e| Error::DbError(DbError::SeaORMError(e.to_string())))?;

    Ok(())
}

pub async fn get(org_id: &str) -> Result<OrganizationRecord, errors::Error> {
    let client = ORM_CLIENT.get_or_init(connect_to_orm).await;
    let record = Entity::find()
        .filter(Column::Identifier.eq(org_id))
        .one(client)
        .await
        .map_err(|e| Error::DbError(DbError::SeaORMError(e.to_string())))?
        .ok_or_else(|| {
            Error::DbError(DbError::SeaORMError("Organization not found".to_string()))
        })?;

    Ok(OrganizationRecord::from(record))
}

pub async fn list(limit: Option<i64>) -> Result<Vec<OrganizationRecord>, errors::Error> {
    let client = ORM_CLIENT.get_or_init(connect_to_orm).await;
    let mut res = Entity::find().order_by(Column::CreatedAt, Order::Desc);
    if let Some(limit) = limit {
        res = res.limit(limit as u64);
    }
    let records = res
        .all(client)
        .await
        .map_err(|e| Error::DbError(DbError::SeaORMError(e.to_string())))?
        .into_iter()
        .map(OrganizationRecord::from)
        .collect();

    Ok(records)
}

pub async fn get_by_name(org_name: &str) -> Result<Vec<OrganizationRecord>, errors::Error> {
    let client = ORM_CLIENT.get_or_init(connect_to_orm).await;
    let records = Entity::find()
        .filter(Column::OrgName.eq(org_name))
        .all(client)
        .await
        .map_err(|e| Error::DbError(DbError::SeaORMError(e.to_string())))?
        .into_iter()
        .map(OrganizationRecord::from)
        .collect();

    Ok(records)
}

pub async fn len() -> usize {
    let client = ORM_CLIENT.get_or_init(connect_to_orm).await;
    let len = Entity::find().count(client).await;

    match len {
        Ok(len) => len as usize,
        Err(e) => {
            log::error!("organizations len error: {e}");
            0
        }
    }
}

pub async fn clear() -> Result<(), errors::Error> {
    // make sure only one client is writing to the database(only for sqlite)
    let _lock = get_lock().await;

    let client = ORM_CLIENT.get_or_init(connect_to_orm).await;
    Entity::delete_many()
        .exec(client)
        .await
        .map_err(|e| Error::DbError(DbError::SeaORMError(e.to_string())))?;

    Ok(())
}

pub async fn is_empty() -> bool {
    len().await == 0
}

pub async fn batch_remove(org_ids: Vec<String>) -> Result<(), errors::Error> {
    // make sure only one client is writing to the database(only for sqlite)
    let _lock = get_lock().await;

    let client = ORM_CLIENT.get_or_init(connect_to_orm).await;
    Entity::delete_many()
        .filter(Column::Identifier.is_in(org_ids))
        .exec(client)
        .await
        .map_err(|e| Error::DbError(DbError::SeaORMError(e.to_string())))?;

    Ok(())
}
