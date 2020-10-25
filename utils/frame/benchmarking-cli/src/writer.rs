// This file is part of Substrate.

// Copyright (C) 2020 Parity Technologies (UK) Ltd.
// SPDX-License-Identifier: Apache-2.0

// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
// 	http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// Outputs benchmark results to Rust files that can be ingested by the runtime.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use serde::Serialize;

use crate::BenchmarkCmd;
use frame_benchmarking::{BenchmarkBatch, BenchmarkSelector, Analysis};
use sp_runtime::traits::Zero;

const VERSION: &'static str = env!("CARGO_PKG_VERSION");
const TEMPLATE: &str = include_str!("./template.hbs");

// This is the final structure we will pass to the Handlebars template.
#[derive(Serialize, Default, Debug)]
struct TemplateData {
	args: Vec<String>,
	date: String,
	version: String,
	pallet: String,
	header: String,
	cmd: CmdData,
	// Map from benchmark name to benchmark data
	benchmarks: HashMap<String, BenchmarkData>,
}

// This was the final data we have about each benchmark.
#[derive(Serialize, Default, Debug, Clone)]
struct BenchmarkData {
	name: String,
	components: Vec<Component>,
	base_weight: u128,
	base_reads: u128,
	base_writes: u128,
	component_weight: Vec<ComponentSlope>,
	component_reads: Vec<ComponentSlope>,
	component_writes: Vec<ComponentSlope>,
}

// This forwards some specific metadata from the `BenchmarkCmd`
#[derive(Serialize, Default, Debug, Clone)]
struct CmdData {
	steps: Vec<u32>,
	repeat: u32,
	lowest_range_values: Vec<u32>,
	highest_range_values: Vec<u32>,
	execution: String,
	wasm_execution: String,
	chain: String,
	db_cache: u32,
}

// This encodes the component name and whether that component is used.
#[derive(Serialize, Debug, Clone, Eq, PartialEq)]
struct Component {
	name: String,
	is_used: bool,
}

// This encodes the slope of some benchmark related to a component.
#[derive(Serialize, Debug, Clone, Eq, PartialEq)]
struct ComponentSlope {
	name: String,
	slope: u128,
}

// Small helper to create an `io::Error` from a string.
fn io_error(s: &str) -> std::io::Error {
	use std::io::{Error, ErrorKind};
	Error::new(ErrorKind::Other, s)
}

// This function takes a list of `BenchmarkBatch` and organizes them by pallet into a `HashMap`.
// So this: `[(p1, b1), (p1, b2), (p1, b3), (p2, b1), (p2, b2)]`
// Becomes:
//
// ```
// p1 -> [b1, b2, b3]
// p2 -> [b1, b2]
// ```
fn map_results(batches: &[BenchmarkBatch]) -> Result<HashMap<String, HashMap<String, BenchmarkData>>, std::io::Error> {
	// Skip if batches is empty.
	if batches.is_empty() { return Err(io_error("empty batches")) }

	let mut all_benchmarks = HashMap::new();
	let mut pallet_map = HashMap::new();

	let mut batches_iter = batches.iter().peekable();
	while let Some(batch) = batches_iter.next() {
		// Skip if there are no results
		if batch.results.is_empty() { continue }

		let pallet_string = String::from_utf8(batch.pallet.clone()).unwrap();
		let benchmark_string = String::from_utf8(batch.benchmark.clone()).unwrap();

		let benchmark_data = get_benchmark_data(batch);
		pallet_map.insert(benchmark_string, benchmark_data);

		// Check if this is the end of the iterator
		if let Some(next) = batches_iter.peek() {
			// Next pallet is different than current pallet, save and create new data.
			let next_pallet = String::from_utf8(next.pallet.clone()).unwrap();
			if next_pallet != pallet_string {
				all_benchmarks.insert(pallet_string, pallet_map.clone());
				pallet_map = HashMap::new();
			}
		} else {
			// This is the end of the iterator, so push the final data.
			all_benchmarks.insert(pallet_string, pallet_map.clone());
		}
	}
	Ok(all_benchmarks)
}

// Analyze and return the relevant results for a given benchmark.
fn get_benchmark_data(batch: &BenchmarkBatch) -> BenchmarkData {
	// Analyze benchmarks to get the linear regression.
	let extrinsic_time = Analysis::min_squares_iqr(&batch.results, BenchmarkSelector::ExtrinsicTime).unwrap();
	let reads = Analysis::min_squares_iqr(&batch.results, BenchmarkSelector::Reads).unwrap();
	let writes = Analysis::min_squares_iqr(&batch.results, BenchmarkSelector::Writes).unwrap();

	// Analysis data may include components that are not used, this filters out anything whose value is zero.
	let mut used_components = Vec::new();
	let mut used_extrinsic_time = Vec::new();
	let mut used_reads = Vec::new();
	let mut used_writes = Vec::new();

	extrinsic_time.slopes.into_iter().zip(extrinsic_time.names.iter()).for_each(|(slope, name)| {
		if !slope.is_zero() {
			if !used_components.contains(&name) { used_components.push(name); }
			used_extrinsic_time.push(ComponentSlope {
				name: name.clone(),
				slope: slope.saturating_mul(1000),
			});
		}
	});
	reads.slopes.into_iter().zip(reads.names.iter()).for_each(|(slope, name)| {
		if !slope.is_zero() {
			if !used_components.contains(&name) { used_components.push(name); }
			used_reads.push(ComponentSlope { name: name.clone(), slope });
		}
	});
	writes.slopes.into_iter().zip(writes.names.iter()).for_each(|(slope, name)| {
		if !slope.is_zero() {
			if !used_components.contains(&name) { used_components.push(name); }
			used_writes.push(ComponentSlope { name: name.clone(), slope });
		}
	});

	// This puts a marker on any component which is entirely unused in the weight formula.
	let components = batch.results[0].components
		.iter()
		.map(|(name, _)| -> Component {
			let name_string = name.to_string();
			let is_used = used_components.contains(&&name_string);
			Component { name: name_string, is_used }
		})
		.collect::<Vec<_>>();

	BenchmarkData {
		name: String::from_utf8(batch.benchmark.clone()).unwrap(),
		components,
		base_weight: extrinsic_time.base.saturating_mul(1000),
		base_reads: reads.base,
		base_writes: writes.base,
		component_weight: used_extrinsic_time,
		component_reads: used_reads,
		component_writes: used_writes,
	}
}

// Create weight file from benchmark data and Handlebars template.
pub fn write_results(
	batches: &[BenchmarkBatch],
	path: &PathBuf,
	cmd: &BenchmarkCmd,
) -> Result<(), std::io::Error> {
	// Use custom template if provided.
	let template: String = match &cmd.template {
		Some(template_file) => {
			fs::read_to_string(template_file)?
		},
		None => {
			println!("Trying default template");
			TEMPLATE.to_string()
		},
	};

	// Use header if provided
	let header_text = match &cmd.header {
		Some(header_file) => {
			let text = fs::read_to_string(header_file)?;
			text
		},
		None => String::new(),
	};

	// Date string metadata
	let date = chrono::Utc::now().format("%Y-%m-%d").to_string();

	// Full CLI args passed to trigger the benchmark.
	let args = std::env::args().collect::<Vec<String>>();

	// Capture individual args
	let cmd_data = CmdData {
		steps: cmd.steps.clone(),
		repeat: cmd.repeat.clone(),
		lowest_range_values: cmd.lowest_range_values.clone(),
		highest_range_values: cmd.highest_range_values.clone(),
		execution: format!("{:?}", cmd.execution),
		wasm_execution: cmd.wasm_method.to_string(),
		chain: format!("{:?}", cmd.shared_params.chain),
		db_cache: cmd.database_cache_size,
	};

	// New Handlebars instance with helpers.
	let mut handlebars = handlebars::Handlebars::new();
	handlebars.register_helper("underscore", Box::new(UnderscoreHelper));
	handlebars.register_helper("join", Box::new(JoinHelper));

	// Organize results by pallet into a JSON map
	let all_results = map_results(batches)?;
	for (pallet, results) in all_results.into_iter() {
		// Create new file: "path/to/pallet_name.rs".
		let mut file_path = path.clone();
		if file_path.file_name().is_none() {
			file_path.push(&pallet);
			file_path.set_extension("rs");
		}

		let hbs_data = TemplateData {
			args: args.clone(),
			date: date.clone(),
			version: VERSION.to_string(),
			pallet: pallet,
			header: header_text.clone(),
			cmd: cmd_data.clone(),
			benchmarks: results,
		};

		let mut output_file = fs::File::create(file_path)?;
		handlebars.render_template_to_write(&template, &hbs_data, &mut output_file)
			.map_err(|e| io_error(&e.to_string()))?;
	}
	Ok(())
}

// Add an underscore after every 3rd character, i.e. a separator for large numbers.
fn underscore<Number>(i: Number) -> String
	where Number: std::string::ToString
{
	let mut s = String::new();
	let i_str = i.to_string();
	let a = i_str.chars().rev().enumerate();
	for (idx, val) in a {
		if idx != 0 && idx % 3 == 0 {
			s.insert(0, '_');
		}
		s.insert(0, val);
	}
	s
}

// A Handlebars helper to add an underscore after every 3rd character,
// i.e. a separator for large numbers.
#[derive(Clone, Copy)]
struct UnderscoreHelper;
impl handlebars::HelperDef for UnderscoreHelper {
	fn call<'reg: 'rc, 'rc>(
		&self, h: &handlebars::Helper,
		_: &handlebars::Handlebars,
		_: &handlebars::Context,
		_rc: &mut handlebars::RenderContext,
		out: &mut dyn handlebars::Output
	) -> handlebars::HelperResult {
		use handlebars::JsonRender;
		let param = h.param(0).unwrap();
		let underscore_param = underscore(param.value().render());
		out.write(&underscore_param)?;
		Ok(())
	}
}

// A helper to join a string of vectors.
#[derive(Clone, Copy)]
struct JoinHelper;
impl handlebars::HelperDef for JoinHelper {
	fn call<'reg: 'rc, 'rc>(
		&self, h: &handlebars::Helper,
		_: &handlebars::Handlebars,
		_: &handlebars::Context,
		_rc: &mut handlebars::RenderContext,
		out: &mut dyn handlebars::Output
	) -> handlebars::HelperResult {
		use handlebars::JsonRender;
		let param = h.param(0).unwrap();
		let value = param.value();
		let joined = if value.is_array() {
			value.as_array().unwrap()
				.iter()
				.map(|v| v.render())
				.collect::<Vec<String>>()
				.join(" ")
		} else {
			value.render()
		};
		out.write(&joined)?;
		Ok(())
	}
}

#[cfg(test)]
mod test {
	use super::*;
	use frame_benchmarking::{BenchmarkBatch, BenchmarkParameter, BenchmarkResults};

	fn test_data(name: Vec<u8>, param: BenchmarkParameter, base: u32, slope: u32) -> BenchmarkBatch {
		let mut results = Vec::new();
		for i in 0 .. 5 {
			results.push(
				BenchmarkResults {
					components: vec![(param, i), (BenchmarkParameter::z, 0)],
					extrinsic_time: (base + slope * i).into(),
					storage_root_time: (base + slope * i).into(),
					reads: (base + slope * i).into(),
					repeat_reads: 0,
					writes: (base + slope * i).into(),
					repeat_writes: 0,
				}
			)
		}

		return BenchmarkBatch {
			pallet: [name.clone(), b"_pallet".to_vec()].concat(),
			benchmark: [name, b"_name".to_vec()].concat(),
			results,
		}

	}

	#[test]
	fn map_results_works() {
		let mapped_results = map_results(&[
			test_data(b"first".to_vec(), BenchmarkParameter::a, 10, 3),
			test_data(b"second".to_vec(), BenchmarkParameter::b, 3, 4),
		]).unwrap();

		let first_benchmark = mapped_results.get("first_pallet").unwrap().get("first_name").unwrap();

		assert_eq!(first_benchmark.name, "first_name");
		assert_eq!(
			first_benchmark.components,
			vec![
				Component { name: "a".to_string(), is_used: true },
				Component { name: "z".to_string(), is_used: false},
			],
		);
		// Weights multiplied by 1,000
		assert_eq!(first_benchmark.base_weight, 10_000);
		assert_eq!(
			first_benchmark.component_weight,
			vec![ComponentSlope { name: "a".to_string(), slope: 3_000 }]
		);
		// DB Reads/Writes are untouched
		assert_eq!(first_benchmark.base_reads, 10);
		assert_eq!(
			first_benchmark.component_reads,
			vec![ComponentSlope { name: "a".to_string(), slope: 3 }]
		);
		assert_eq!(first_benchmark.base_writes, 10);
		assert_eq!(
			first_benchmark.component_writes,
			vec![ComponentSlope { name: "a".to_string(), slope: 3 }]
		);

		let second_benchmark = mapped_results.get("second_pallet").unwrap().get("second_name").unwrap();

		assert_eq!(second_benchmark.name, "second_name");
		assert_eq!(
			second_benchmark.components,
			vec![
				Component { name: "b".to_string(), is_used: true },
				Component { name: "z".to_string(), is_used: false},
			],
		);
		// Weights multiplied by 1,000
		assert_eq!(second_benchmark.base_weight, 3_000);
		assert_eq!(
			second_benchmark.component_weight,
			vec![ComponentSlope { name: "b".to_string(), slope: 4_000 }]
		);
		// DB Reads/Writes are untouched
		assert_eq!(second_benchmark.base_reads, 3);
		assert_eq!(
			second_benchmark.component_reads,
			vec![ComponentSlope { name: "b".to_string(), slope: 4 }]
		);
		assert_eq!(second_benchmark.base_writes, 3);
		assert_eq!(
			second_benchmark.component_writes,
			vec![ComponentSlope { name: "b".to_string(), slope: 4 }]
		);
	}
}
