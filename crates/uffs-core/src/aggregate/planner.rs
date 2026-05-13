// SPDX-License-Identifier: MPL-2.0
// Copyright (c) 2025-2026 SKY, LLC.

//! Aggregate plan compilation.
//!
//! The [`AggregatePlan`] compiles a set of [`AggregateSpec`]s into an
//! execution plan that the engine uses to create and manage accumulators.

use super::AggregateError;
use super::accumulators::GroupAccumulator;
use super::spec::{AggregateKind, AggregateSpec};

/// A compiled aggregate execution plan.
///
/// Created from a slice of [`AggregateSpec`]s, the plan validates field
/// compatibility and organises specs by cost tier for efficient execution.
#[derive(Debug)]
pub struct AggregatePlan {
    /// The validated specs in execution order.
    pub(crate) specs: Vec<AggregateSpec>,
}

impl AggregatePlan {
    /// Compile a set of aggregate specs into an execution plan.
    ///
    /// Validates that each spec's field supports the requested operation
    /// using the field's `AggregateMeta`.
    ///
    /// # Errors
    ///
    /// Returns `AggregateError::UnsupportedField` if a field doesn't
    /// support the requested aggregate operation.
    pub fn compile(specs: &[AggregateSpec]) -> Result<Self, AggregateError> {
        let mut validated = Vec::with_capacity(specs.len());

        for spec in specs {
            Self::validate_spec(spec)?;
            validated.push(spec.clone());
        }

        Ok(Self { specs: validated })
    }

    /// Create a fresh set of accumulators for this plan.
    #[must_use]
    pub(crate) fn create_accumulators(&self) -> Vec<GroupAccumulator> {
        self.specs
            .iter()
            .map(|spec| GroupAccumulator::from_kind(&spec.kind, spec.label.clone()))
            .collect()
    }

    /// Number of specs in this plan.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.specs.len()
    }

    /// Whether this plan has no specs.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.specs.is_empty()
    }

    /// Validate that a spec's field supports the requested operation.
    fn validate_spec(spec: &AggregateSpec) -> Result<(), AggregateError> {
        match &spec.kind {
            AggregateKind::Count
            | AggregateKind::Missing { .. }
            | AggregateKind::Rollup { .. }
            | AggregateKind::Duplicates { .. } => Ok(()),

            AggregateKind::Stats { field, .. } => {
                let meta = field.metadata();
                if !meta.aggregate.aggregatable {
                    return Err(AggregateError::UnsupportedField {
                        field: meta.canonical_name.to_owned(),
                        operation: "stats (sum/min/max/avg)".to_owned(),
                    });
                }
                Ok(())
            }

            AggregateKind::Terms { field, .. } => {
                let meta = field.metadata();
                if !meta.aggregate.groupable {
                    return Err(AggregateError::UnsupportedField {
                        field: meta.canonical_name.to_owned(),
                        operation: "terms (group-by)".to_owned(),
                    });
                }
                Ok(())
            }

            AggregateKind::Histogram { field, .. } | AggregateKind::Range { field, .. } => {
                let meta = field.metadata();
                if !meta.aggregate.bucket_support {
                    return Err(AggregateError::UnsupportedField {
                        field: meta.canonical_name.to_owned(),
                        operation: "histogram/range (bucket)".to_owned(),
                    });
                }
                Ok(())
            }

            AggregateKind::DateHistogram { field, .. } => {
                let meta = field.metadata();
                if !meta.aggregate.bucket_support {
                    return Err(AggregateError::UnsupportedField {
                        field: meta.canonical_name.to_owned(),
                        operation: "date_histogram".to_owned(),
                    });
                }
                Ok(())
            }

            AggregateKind::Distinct { field } => {
                let meta = field.metadata();
                if !meta.aggregate.groupable {
                    return Err(AggregateError::UnsupportedField {
                        field: meta.canonical_name.to_owned(),
                        operation: "distinct count".to_owned(),
                    });
                }
                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aggregate::spec::{BucketMetric, ScalarMetric};
    use crate::search::field::FieldId;

    #[test]
    fn compile_valid_count() {
        let specs = [AggregateSpec::new(AggregateKind::Count)];
        let plan = AggregatePlan::compile(&specs).expect("count should compile");
        assert_eq!(plan.len(), 1);
    }

    #[test]
    fn compile_valid_stats() {
        let specs = [AggregateSpec::new(AggregateKind::Stats {
            field: FieldId::Size,
            metrics: vec![ScalarMetric::Sum],
        })];
        let plan = AggregatePlan::compile(&specs).expect("size stats should compile");
        assert_eq!(plan.len(), 1);
    }

    #[test]
    fn compile_valid_terms() {
        let specs = [AggregateSpec::new(AggregateKind::Terms {
            field: FieldId::Extension,
            top: 50,
            metrics: vec![BucketMetric::Count],
            sample: None,
        })];
        let plan = AggregatePlan::compile(&specs).expect("ext terms should compile");
        assert_eq!(plan.len(), 1);
    }
}
