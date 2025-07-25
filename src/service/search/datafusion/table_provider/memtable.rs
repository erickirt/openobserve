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

use std::{any::Any, sync::Arc};

use arrow::array::RecordBatch;
use arrow_schema::{DataType, SchemaRef, SortOptions};
use async_trait::async_trait;
use config::get_config;
use datafusion::{
    catalog::Session,
    common::{Constraints, Result},
    datasource::{MemTable, TableProvider},
    logical_expr::{Expr, TableType},
    physical_expr::{LexOrdering, PhysicalSortExpr},
    physical_plan::{ExecutionPlan, expressions::Column, sorts::sort::SortExec},
};
use hashbrown::HashMap;

use super::{apply_filter, apply_projection};
use crate::service::search::index::IndexCondition;

#[derive(Debug)]
pub(crate) struct NewMemTable {
    mem_table: MemTable,
    diff_rules: HashMap<String, DataType>,
    sorted_by_time: bool,
    index_condition: Option<IndexCondition>,
    fst_fields: Vec<String>,
}

impl NewMemTable {
    /// Create a new in-memory table from the provided schema and record batches
    pub fn try_new(
        schema: SchemaRef,
        partitions: Vec<Vec<RecordBatch>>,
        rules: HashMap<String, DataType>,
        sorted_by_time: bool,
        index_condition: Option<IndexCondition>,
        fst_fields: Vec<String>,
    ) -> Result<Self> {
        let mem = MemTable::try_new(schema, partitions)?;
        Ok(Self {
            mem_table: mem,
            diff_rules: rules,
            sorted_by_time,
            index_condition,
            fst_fields,
        })
    }
}

#[async_trait]
impl TableProvider for NewMemTable {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn schema(&self) -> SchemaRef {
        self.mem_table.schema()
    }

    fn constraints(&self) -> Option<&Constraints> {
        self.mem_table.constraints()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> Result<Arc<dyn ExecutionPlan>> {
        let (mem_projection, filter_projection) =
            if let Some(index_condition) = self.index_condition.as_ref() {
                // get the projection for the filter
                let mut filter_projection =
                    index_condition.get_schema_projection(self.schema(), &self.fst_fields);
                if let Some(v) = projection.as_ref() {
                    filter_projection.extend(v.iter().copied());
                }
                filter_projection.sort();
                filter_projection.dedup();
                // regenerate the projection with the filter_projection
                let projection = projection.as_ref().map(|p| {
                    p.iter()
                        .filter_map(|i| filter_projection.iter().position(|f| f == i))
                        .collect::<Vec<_>>()
                });
                (Some(filter_projection), projection)
            } else {
                (projection.cloned(), None)
            };
        let mem_projection = mem_projection.as_ref();
        let filter_projection = filter_projection.as_ref();

        let memory_exec = self
            .mem_table
            .scan(state, mem_projection, filters, limit)
            .await?;

        let projection_exec = apply_projection(
            &self.schema(),
            &self.diff_rules,
            mem_projection,
            memory_exec,
        )?;

        // if the index condition can remove filter, we can skip the config
        // feature_query_remove_filter_with_index
        let can_remove_filter = self
            .index_condition
            .as_ref()
            .map(|v| v.can_remove_filter())
            .unwrap_or(true);
        let filter_exec =
            if can_remove_filter || get_config().common.feature_query_remove_filter_with_index {
                apply_filter(
                    self.index_condition.as_ref(),
                    &projection_exec.schema(),
                    &self.fst_fields,
                    projection_exec,
                    filter_projection,
                )?
            } else {
                projection_exec
            };

        apply_sort(filter_exec, self.sorted_by_time)
    }

    fn get_column_default(&self, column: &str) -> Option<&Expr> {
        self.mem_table.get_column_default(column)
    }
}

// create sort exec by _timestamp
fn wrap_sort(exec: Arc<dyn ExecutionPlan>) -> Arc<dyn ExecutionPlan> {
    let column_timestamp = config::TIMESTAMP_COL_NAME.to_string();
    let index = exec.schema().index_of(&column_timestamp);
    (match index {
        Ok(index) => {
            let ordering = LexOrdering::new(vec![PhysicalSortExpr {
                expr: Arc::new(Column::new(&column_timestamp, index)),
                options: SortOptions {
                    descending: true,
                    nulls_first: false,
                },
            }]);
            Arc::new(SortExec::new(ordering, exec))
        }
        Err(_) => exec,
    }) as _
}

fn apply_sort(
    exec_plan: Arc<dyn ExecutionPlan>,
    sorted_by_time: bool,
) -> Result<Arc<dyn ExecutionPlan>> {
    Ok(if sorted_by_time {
        wrap_sort(exec_plan)
    } else {
        exec_plan
    })
}
