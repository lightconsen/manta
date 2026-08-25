//! `generate_chart` — render a data chart (bar / line / pie) to SVG + PNG.
//!
//! Uses charts-rs for precise, deterministic chart rendering (the LLM supplies
//! structured data, not hand-computed coordinates), then rasterizes the SVG
//! to PNG so the chart can be embedded into any document format.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::Value;

use super::{create_schema, Tool, ToolContext, ToolExecutionResult};
use crate::tools::report::resolve_artifact_target;
use crate::tools::sdk::ToolCapabilities;
use crate::tools::svg_to_png::rasterize_svg;

/// One chart series: a name plus its data values.
#[derive(Debug, Deserialize)]
struct SeriesParam {
    name: String,
    data: Vec<f64>,
}

/// Render a chart to an SVG string.
fn chart_to_svg(
    chart_type: &str,
    title: &str,
    series: &[SeriesParam],
    categories: &[String],
    width: Option<f32>,
    height: Option<f32>,
) -> Result<String, String> {
    use charts_rs::{BarChart, LineChart, PieChart, Series};

    let series_list: Vec<Series> = series
        .iter()
        .map(|s| {
            let data: Vec<f32> = s.data.iter().map(|v| *v as f32).collect();
            (s.name.as_str(), data).into()
        })
        .collect();

    let svg = match chart_type {
        "bar" => {
            let mut c = BarChart::new(series_list, categories.to_vec());
            if !title.is_empty() {
                c.title_text = title.to_string();
            }
            if let Some(w) = width {
                c.width = w;
            }
            if let Some(h) = height {
                c.height = h;
            }
            c.svg().map_err(|e| e.to_string())?
        }
        "line" => {
            let mut c = LineChart::new(series_list, categories.to_vec());
            if !title.is_empty() {
                c.title_text = title.to_string();
            }
            if let Some(w) = width {
                c.width = w;
            }
            if let Some(h) = height {
                c.height = h;
            }
            c.svg().map_err(|e| e.to_string())?
        }
        "pie" => {
            // Each series is one slice (name + single value).
            let mut c = PieChart::new(series_list);
            if !title.is_empty() {
                c.title_text = title.to_string();
            }
            if let Some(w) = width {
                c.width = w;
            }
            if let Some(h) = height {
                c.height = h;
            }
            c.svg().map_err(|e| e.to_string())?
        }
        _ => {
            return Err(format!(
                "unsupported chart_type '{chart_type}' (supported: bar, line, pie)"
            ))
        }
    };
    Ok(svg)
}

/// `generate_chart` tool.
#[derive(Debug, Default)]
pub struct GenerateChartTool;

impl GenerateChartTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for GenerateChartTool {
    fn name(&self) -> &str {
        "generate_chart"
    }

    fn description(&self) -> &str {
        "Generate a data chart (bar, line, or pie) from structured data and \
         render it to SVG + PNG. \
         \
         Use this for precise, data-accurate charts where the axis and values \
         must be correct — do NOT hand-write chart SVG. Supply `series` (name + \
         numeric data) and, for bar/line, `categories` (x-axis labels). \
         \
         The returned `filename` can be `<img>`-referenced in a slides/docx \
         document, or `png_url` in markdown/html."
    }

    fn parameters_schema(&self) -> Value {
        create_schema(
            "Generate a data chart (SVG + PNG)",
            serde_json::json!({
                "chart_type": {
                    "type": "string",
                    "enum": ["bar", "line", "pie"],
                    "description": "Chart type."
                },
                "title": {
                    "type": "string",
                    "description": "Chart title."
                },
                "series": {
                    "type": "array",
                    "description": "Data series. For bar/line each entry is {name, data: [numbers]}; for pie each entry is one slice {name, data: [value]}.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "name": { "type": "string" },
                            "data": { "type": "array", "items": { "type": "number" } }
                        },
                        "required": ["name", "data"]
                    }
                },
                "categories": {
                    "type": "array",
                    "description": "X-axis labels for bar/line charts.",
                    "items": { "type": "string" }
                },
                "width": { "type": "number", "description": "Chart width in px (optional)." },
                "height": { "type": "number", "description": "Chart height in px (optional)." },
                "filename": {
                    "type": "string",
                    "description": "Base filename (no extension), e.g. \"sales-chart\". Defaults to a generated name."
                }
            }),
            vec!["chart_type", "series"],
        )
    }

    fn capabilities(&self) -> ToolCapabilities {
        ToolCapabilities {
            requires_approval: false,
            risk_level: crate::tools::approval::RiskLevel::Low,
            categories: vec!["image".to_string(), "content".to_string()],
            ..Default::default()
        }
    }

    async fn execute(
        &self,
        args: Value,
        context: &ToolContext,
    ) -> crate::Result<ToolExecutionResult> {
        let chart_type = args["chart_type"].as_str().unwrap_or("bar");
        let title = args["title"].as_str().unwrap_or("").to_string();
        let series: Vec<SeriesParam> =
            serde_json::from_value(args["series"].clone()).map_err(|e| {
                crate::error::SyscityError::Validation(format!("Invalid 'series': {e}"))
            })?;
        if series.is_empty() {
            return Err(crate::error::SyscityError::Validation(
                "'series' must contain at least one entry".to_string(),
            ));
        }
        let categories: Vec<String> = args["categories"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let width = args["width"].as_f64().map(|v| v as f32);
        let height = args["height"].as_f64().map(|v| v as f32);

        let svg = chart_to_svg(chart_type, &title, &series, &categories, width, height)
            .map_err(crate::error::SyscityError::Validation)?;
        let (png, w, h) = rasterize_svg(&svg).map_err(crate::error::SyscityError::Validation)?;

        let base = args["filename"]
            .as_str()
            .map(|s| {
                s.trim_end_matches(".svg")
                    .trim_end_matches(".png")
                    .to_string()
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("chart-{}", uuid::Uuid::new_v4()));
        let svg_filename = format!("{base}.svg");
        let png_filename = format!("{base}.png");

        let (artifacts_dir, _) = resolve_artifact_target(context, &png_filename);
        tokio::fs::create_dir_all(&artifacts_dir)
            .await
            .map_err(|e| crate::error::SyscityError::IoContext {
                context: "Failed to create artifacts directory".to_string(),
                source: e,
            })?;
        tokio::fs::write(artifacts_dir.join(&svg_filename), svg.as_bytes())
            .await
            .map_err(|e| crate::error::SyscityError::IoContext {
                context: format!("Failed to write {svg_filename}"),
                source: e,
            })?;
        tokio::fs::write(artifacts_dir.join(&png_filename), &png)
            .await
            .map_err(|e| crate::error::SyscityError::IoContext {
                context: format!("Failed to write {png_filename}"),
                source: e,
            })?;

        let (_, svg_url) = resolve_artifact_target(context, &svg_filename);
        let (_, png_url) = resolve_artifact_target(context, &png_filename);

        Ok(ToolExecutionResult::success(format!("Generated {chart_type} chart ({w}x{h})"))
            .with_data(serde_json::json!({
                "svg_url": svg_url,
                "png_url": png_url,
                "filename": png_filename,
                "width": w,
                "height": h,
            })))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chart_to_svg_bar_line_pie() {
        let series = vec![
            SeriesParam {
                name: "A".to_string(),
                data: vec![1.0, 2.0, 3.0],
            },
            SeriesParam {
                name: "B".to_string(),
                data: vec![2.0, 4.0, 6.0],
            },
        ];
        let cats = vec!["x1".to_string(), "x2".to_string(), "x3".to_string()];

        let bar = chart_to_svg("bar", "T", &series, &cats, None, None).unwrap();
        assert!(bar.contains("<svg"), "bar svg missing root");
        let line = chart_to_svg("line", "T", &series, &cats, None, None).unwrap();
        assert!(line.contains("<svg"));

        let pie_series = vec![
            SeriesParam {
                name: "A".to_string(),
                data: vec![30.0],
            },
            SeriesParam {
                name: "B".to_string(),
                data: vec![70.0],
            },
        ];
        let pie = chart_to_svg("pie", "T", &pie_series, &[], None, None).unwrap();
        assert!(pie.contains("<svg"));

        assert!(chart_to_svg("bogus", "T", &series, &cats, None, None).is_err());
    }

    #[test]
    fn generated_chart_rasterizes() {
        let series = vec![SeriesParam {
            name: "A".to_string(),
            data: vec![10.0, 20.0],
        }];
        let cats = vec!["a".to_string(), "b".to_string()];
        let svg = chart_to_svg("bar", "Test", &series, &cats, Some(400.0), Some(300.0)).unwrap();
        let (png, w, h) = rasterize_svg(&svg).unwrap();
        assert_eq!(&png[..8], &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(w, 400);
        assert_eq!(h, 300);
    }
}
