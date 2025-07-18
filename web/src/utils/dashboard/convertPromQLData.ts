// Copyright 2023 OpenObserve Inc.
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

import {
  calculateOptimalFontSize,
  formatDate,
  formatUnitValue,
  getContrastColor,
  getUnitValue,
} from "./convertDataIntoUnitValue";
import { toZonedTime } from "date-fns-tz";
import { calculateGridPositions } from "./calculateGridForSubPlot";
import {
  ColorModeWithoutMinMax,
  getMetricMinMaxValue,
  getSeriesColor,
} from "./colorPalette";
import { getAnnotationsData } from "@/utils/dashboard/getAnnotationsData";

let moment: any;
let momentInitialized = false;

const importMoment = async () => {
  if (!momentInitialized) {
    const momentModule: any = await import("moment");
    moment = momentModule.default;
    momentInitialized = true;
  }

  return moment;
};

const getMarkLineData = (panelSchema: any) => {
  return (
    panelSchema?.config?.mark_line?.map((markLine: any) => {
      return {
        name: markLine.name,
        type: markLine.type,
        xAxis: markLine.type == "xAxis" ? markLine.value : null,
        yAxis: markLine.type == "yAxis" ? markLine.value : null,
        label: {
          formatter: markLine.name ? "{b}:{c}" : "{c}",
          position: "insideEndTop",
        },
      };
    }) ?? []
  );
};

/**
 * Converts PromQL data into a format suitable for rendering a chart.
 *
 * @param {any} panelSchema - the panel schema object
 * @param {any} searchQueryData - the search query data
 * @param {any} store - the store object
 * @return {Object} - the option object for rendering the chart
 */
export const convertPromQLData = async (
  panelSchema: any,
  searchQueryData: any,
  store: any,
  chartPanelRef: any,
  hoveredSeriesState: any,
  annotations: any,
) => {
  // console.time("convertPromQLData");

  await importMoment();

  // if no data than return it
  if (
    !Array.isArray(searchQueryData) ||
    searchQueryData.length === 0 ||
    !searchQueryData[0] ||
    !panelSchema
  ) {
    // console.timeEnd("convertPromQLData");
    return { options: null };
  }

  // Initialize extras object
  let extras: any = {};

  // get the limit series from the config
  let limitSeries = store.state?.zoConfig?.max_dashboard_series ?? 100;

  // get the total series
  let totalSeries = 0;
  searchQueryData.forEach((queryData: any) => {
    totalSeries += queryData.result?.length || 0;
  });

  // Limit number of series to limitSeries
  const limitedSearchQueryData = searchQueryData.map((queryData: any) => {
    const remainingSeries = queryData.result?.slice(0, limitSeries);
    limitSeries = limitSeries - remainingSeries.length;
    return {
      ...queryData,
      result: remainingSeries,
    };
  });

  // Add warning if total number of series exceeds limit
  if (totalSeries > (store.state?.zoConfig?.max_dashboard_series ?? 100)) {
    extras.limitNumberOfSeriesWarningMessage =
      "Limiting the displayed series to ensure optimal performance";
  }

  // flag to check if the data is time seriesc
  let isTimeSeriesFlag = true;

  const legendPosition = getLegendPosition(
    panelSchema?.config?.legends_position,
  );

  // get the x axis key which will be timestamp
  let xAxisData: any = new Set();

  // add all series timestamp
  limitedSearchQueryData.forEach((queryData: any) =>
    queryData.result.forEach((result: any) =>
      result.values.forEach((value: any) => xAxisData.add(value[0])),
    ),
  );

  // sort the timestamp and make an array
  xAxisData = Array.from(xAxisData).sort();

  // convert timestamp to specified timezone time
  xAxisData.forEach((value: number, index: number) => {
    // we need both milliseconds and date (object or string)
    xAxisData[index] = [
      value,
      store.state.timezone != "UTC"
        ? toZonedTime(value * 1000, store.state.timezone)
        : new Date(value * 1000).toISOString().slice(0, -1),
    ];
  });

  const legendConfig: any = {
    show: panelSchema.config?.show_legends,
    type: "scroll",
    orient: legendPosition,
    padding: [10, 20, 10, 10],
    tooltip: {
      show: true,
      padding: 10,
      textStyle: {
        fontSize: 12,
      },
      formatter: (params: any) => {
        hoveredSeriesState?.value?.setHoveredSeriesName(params?.name);
        return params?.name;
      },
    },
    textStyle: {
      width: 150,
      overflow: "truncate",
      rich: {
        a: {
          fontWeight: "bold",
        },
        b: {
          fontStyle: "normal",
        },
      },
    },
    formatter: (name: any) => {
      return name == hoveredSeriesState?.value?.hoveredSeriesName
        ? "{a|" + name + "}"
        : "{b|" + name + "}";
    },
  };

  // Additional logic to adjust the legend position
  if (legendPosition === "vertical") {
    legendConfig.left = null; // Remove left positioning
    legendConfig.right = 0; // Apply right positioning
    legendConfig.top = "center"; // Apply bottom positioning
  } else {
    legendConfig.left = "0"; // Apply left positioning
    legendConfig.top = "bottom"; // Apply bottom positioning
  }

  const { markLines, markAreas } = getAnnotationsData(
    annotations,
    store.state.timezone,
  );

  const getSeriesMarkArea = () => {
    return {
      itemStyle: {
        color: "rgba(0, 191, 255, 0.15)",
      },
      data: markAreas,
    };
  };

  const [min, max] = getMetricMinMaxValue(limitedSearchQueryData);

  const getFinalAxisValue = (
    configValue: number | null | undefined,
    dataValue: number,
    isMin: boolean,
  ) => {
    if (configValue === null || configValue === undefined) {
      return undefined;
    }
    return isMin
      ? Math.min(configValue, dataValue)
      : Math.max(configValue, dataValue);
  };

  const options: any = {
    backgroundColor: "transparent",
    legend: legendConfig,
    grid: {
      containLabel: panelSchema.config?.axis_width == null ? true : false,
      //based on config width set grid
      left: panelSchema.config?.axis_width ?? 5,
      right: 20,
      top: "15",
      bottom:
        legendConfig.orient === "horizontal" && panelSchema.config?.show_legends
          ? panelSchema.config?.axis_width == null
            ? 30
            : 50
          : panelSchema.config?.axis_width == null
            ? 5
            : 25,
    },
    tooltip: {
      show: true,
      trigger: "axis",
      textStyle: {
        color: store.state.theme === "dark" ? "#fff" : "#000",
        fontSize: 12,
      },
      enterable: true,
      backgroundColor:
        store.state.theme === "dark" ? "rgba(0,0,0,1)" : "rgba(255,255,255,1)",
      extraCssText:
        "max-height: 200px; overflow: auto; max-width: 500px; user-select: text;",
      formatter: function (name: any) {
        // show tooltip for hovered panel only for other we only need axis so just return empty string
        if (
          hoveredSeriesState?.value &&
          panelSchema.id &&
          hoveredSeriesState?.value?.panelId != panelSchema.id
        )
          return "";

        if (name.length == 0) return "";

        const date = new Date(name[0].data[0]);

        // sort tooltip array based on value
        name.sort((a: any, b: any) => {
          return (b.value[1] || 0) - (a.value[1] || 0);
        });

        // if hovered series name is not null then move it to first position
        if (hoveredSeriesState?.value?.hoveredSeriesName) {
          // get the current series index from name
          const currentSeriesIndex = name.findIndex(
            (it: any) =>
              it.seriesName == hoveredSeriesState?.value?.hoveredSeriesName,
          );

          // if hovered series index is not -1 then take it to very first position
          if (currentSeriesIndex != -1) {
            // shift all series to next position and place current series at first position
            const temp = name[currentSeriesIndex];
            for (let i = currentSeriesIndex; i > 0; i--) {
              name[i] = name[i - 1];
            }
            name[0] = temp;
          }
        }

        const hoverText: string[] = [];
        name.forEach((it: any) => {
          // if data is not null than show in tooltip
          if (it.data[1] != null) {
            // check if the series is the current series being hovered
            // if have than bold it
            if (it?.seriesName == hoveredSeriesState?.value?.hoveredSeriesName)
              hoverText.push(
                `<strong>${it.marker} ${it.seriesName} : ${formatUnitValue(
                  getUnitValue(
                    it.data[1],
                    panelSchema.config?.unit,
                    panelSchema.config?.unit_custom,
                    panelSchema.config?.decimals,
                  ),
                )} </strong>`,
              );
            // else normal text
            else
              hoverText.push(
                `${it.marker} ${it.seriesName} : ${formatUnitValue(
                  getUnitValue(
                    it.data[1],
                    panelSchema.config?.unit,
                    panelSchema.config?.unit_custom,
                    panelSchema.config?.decimals,
                  ) ?? "",
                )}`,
              );
          }
        });

        return `${formatDate(date)} <br/> ${hoverText.join("<br/>")}`;
      },
      axisPointer: {
        show: true,
        type: "cross",
        label: {
          fontSize: 12,
          precision: panelSchema.config?.decimals,
          show: true,
          formatter: function (name: any) {
            if (name.axisDimension == "y")
              return formatUnitValue(
                getUnitValue(
                  name.value,
                  panelSchema.config?.unit,
                  panelSchema.config?.unit_custom,
                  panelSchema.config?.decimals,
                ),
              );
            const date = new Date(name.value);
            return `${formatDate(date)}`;
          },
        },
      },
    },
    xAxis: {
      type: "time",
      axisLine: {
        show: searchQueryData?.every((it: any) => it.result.length == 0)
          ? true
          : (panelSchema.config?.axis_border_show ?? false),
      },
      splitLine: {
        show: true,
        lineStyle: {
          opacity: 0.5,
        },
      },
      axisLabel: {
        // hide axis label if overlaps
        hideOverlap: true,
      },
    },
    yAxis: {
      type: "value",
      min: getFinalAxisValue(panelSchema.config.y_axis_min, min, true),
      max: getFinalAxisValue(panelSchema.config.y_axis_max, max, false),
      axisLabel: {
        formatter: function (name: any) {
          return formatUnitValue(
            getUnitValue(
              name,
              panelSchema.config?.unit,
              panelSchema.config?.unit_custom,
              panelSchema.config?.decimals,
            ),
          );
        },
      },
      axisLine: {
        show: searchQueryData?.every((it: any) => it.result.length == 0)
          ? true
          : (panelSchema.config?.axis_border_show ?? false),
      },
      splitLine: {
        show: true,
        lineStyle: {
          opacity: 0.5,
        },
      },
    },
    toolbox: {
      orient: "vertical",
      show: !["pie", "donut", "metric", "gauge"].includes(panelSchema.type),
      showTitle: false,
      tooltip: {
        show: false,
      },
      itemSize: 0,
      itemGap: 0,
      // it is used to hide toolbox buttons
      bottom: "100%",
      feature: {
        dataZoom: {
          filterMode: "none",
          yAxisIndex: "none",
        },
      },
    },
    series: [],
  };
  // to pass grid index in gauge chart
  let gaugeIndex = 0;

  // for gauge chart we need total no. of gauge to calculate grid positions
  let totalLength = 0;
  // for gauge chart, it contains grid array, single chart height and width, no. of charts per row and no. of columns
  let gridDataForGauge: any = {};

  if (panelSchema.type === "gauge") {
    // calculate total length of all metrics
    limitedSearchQueryData.forEach((metric: any) => {
      if (metric.result && Array.isArray(metric.result)) {
        totalLength += metric.result.length;
      }
    });

    // create grid array based on chart panel width, height and total no. of gauge
    gridDataForGauge = calculateGridPositions(
      chartPanelRef.value.offsetWidth,
      chartPanelRef.value.offsetHeight,
      totalLength,
    );

    //assign grid array to gauge chart options
    options.grid = gridDataForGauge.gridArray;
  }

  const seriesPropsBasedOnChartType = getPropsByChartTypeForSeries(
    panelSchema.type,
  );

  // if color type is shades, continuous then required to calculate min and max for chart.
  let chartMin: any = Infinity;
  let chartMax: any = -Infinity;

  if (
    !Object.values(ColorModeWithoutMinMax).includes(
      panelSchema.config?.color?.mode,
    )
  ) {
    [chartMin, chartMax] = getMetricMinMaxValue(limitedSearchQueryData);
  }

  options.series = limitedSearchQueryData.map((it: any, index: number) => {
    switch (panelSchema.type) {
      case "bar":
      case "line":
      case "area":
      case "scatter":
      case "area-stacked": {
        switch (it?.resultType) {
          case "matrix": {
            const seriesObj = it?.result?.map((metric: any) => {
              // Now, we are using xaxisData which will be sorted by the timestamp
              // const values = metric.values.sort(
              //   (a: any, b: any) => a[0] - b[0]
              // );

              // object, which will have timestamp as key and value as value
              const seriesDataObj: any = {};
              metric.values.forEach((value: any) => {
                seriesDataObj[value[0]] = value[1];
              });

              const seriesName = getPromqlLegendName(
                metric.metric,
                panelSchema.queries[index].config.promql_legend,
              );

              return {
                name: seriesName,
                label: {
                  show: panelSchema.config?.label_option?.position != null,
                  position:
                    panelSchema.config?.label_option?.position || "None",
                  rotate: panelSchema.config?.label_option?.rotate || 0,
                },
                smooth:
                  panelSchema.config?.line_interpolation === "smooth" ||
                  panelSchema.config?.line_interpolation == null,
                step: ["step-start", "step-end", "step-middle"].includes(
                  panelSchema.config?.line_interpolation,
                )
                  ? // TODO: replace this with type integrations
                    panelSchema.config.line_interpolation.replace("step-", "")
                  : false,
                showSymbol: panelSchema.config?.show_symbol ?? false,
                zlevel: 2,
                itemStyle: {
                  color: (() => {
                    try {
                      return getSeriesColor(
                        panelSchema?.config?.color,
                        seriesName,
                        metric.values.map((value: any) => value[1]),
                        chartMin,
                        chartMax,
                        store.state.theme,
                      );
                    } catch (error) {
                      console.warn("Failed to get series color:", error);
                      return undefined; // fallback to default color
                    }
                  })(),
                },
                // if utc then simply return the values by removing z from string
                // else convert time from utc to zoned
                // used slice to remove Z from isostring to pass as a utc
                data: xAxisData.map((value: any) => [
                  // value will be an array [milliseconds, date object or date string]
                  value[1],
                  seriesDataObj[value[0]] ?? null,
                ]),
                ...seriesPropsBasedOnChartType,
                // markLine if exist
                markLine: {
                  silent: true,
                  animation: false,
                  data: getMarkLineData(panelSchema),
                },
                connectNulls: panelSchema.config?.connect_nulls ?? false,
              };
            });

            return seriesObj;
          }
          case "vector": {
            const traces = it?.result?.map((metric: any) => {
              const values = [metric.value];
              return {
                name: JSON.stringify(metric.metric),
                x: values.map((value: any) =>
                  moment(value[0] * 1000).toISOString(true),
                ),
                y: values.map((value: any) => value[1]),
              };
            });
            return traces;
          }
        }
      }
      case "gauge": {
        // we doesnt required to hover timeseries for gauge chart
        isTimeSeriesFlag = false;
        const series = it?.result?.map((metric: any) => {
          const values = metric.values.sort((a: any, b: any) => a[0] - b[0]);
          gaugeIndex++;

          const seriesName = getPromqlLegendName(
            metric.metric,
            panelSchema.queries[index].config.promql_legend,
          );

          return {
            ...getPropsByChartTypeForSeries(panelSchema.type),
            min: panelSchema?.queries[index]?.config?.min || 0,
            max: panelSchema?.queries[index]?.config?.max || 100,
            //which grid will be used
            gridIndex: gaugeIndex - 1,
            // radius, progress and axisline width will be calculated based on grid width and height
            radius: `${Math.min(gridDataForGauge.gridWidth, gridDataForGauge.gridHeight) / 2 - 5}px`,
            progress: {
              show: true,
              width: `${Math.min(gridDataForGauge.gridWidth, gridDataForGauge.gridHeight) / 6}`,
            },
            axisLine: {
              lineStyle: {
                width: `${Math.min(gridDataForGauge.gridWidth, gridDataForGauge.gridHeight) / 6}`,
              },
            },
            title: {
              fontSize: 10,
              offsetCenter: [0, "70%"],
              width: `${gridDataForGauge.gridWidth}`,
              overflow: "truncate",
            },
            center: [
              `${parseFloat(options.grid[gaugeIndex - 1].left) + parseFloat(options.grid[gaugeIndex - 1].width) / 2}%`,
              `${parseFloat(options.grid[gaugeIndex - 1].top) + parseFloat(options.grid[gaugeIndex - 1].height) / 2}%`,
            ],
            data: [
              {
                name: seriesName,
                // taking first value for gauge
                value: values[0][1],
                detail: {
                  formatter: function (value: any) {
                    const unitValue = getUnitValue(
                      value,
                      panelSchema.config?.unit,
                      panelSchema.config?.unit_custom,
                      panelSchema.config?.decimals,
                    );
                    return unitValue.value + unitValue.unit;
                  },
                },
                itemStyle: {
                  color: (() => {
                    const defaultColor = null;
                    if (!values?.[0]?.[1]) return defaultColor;
                    return (
                      getSeriesColor(
                        panelSchema?.config?.color,
                        seriesName,
                        values[0][1],
                        chartMin,
                        chartMax,
                        store.state.theme,
                      ) ?? defaultColor
                    );
                  })(),
                },
              },
            ],
            detail: {
              valueAnimation: true,
              offsetCenter: [0, 0],
              fontSize: 12,
            },
          };
        });

        options.dataset = { source: [[]] };
        options.tooltip = {
          show: true,
          trigger: "item",
          textStyle: {
            color: store.state.theme === "dark" ? "#fff" : "#000",
            fontSize: 12,
          },
          valueFormatter: (value: any) => {
            return formatUnitValue(
              getUnitValue(
                value,
                panelSchema.config?.unit,
                panelSchema.config?.unit_custom,
                panelSchema.config?.decimals,
              ),
            );
          },
          enterable: true,
          backgroundColor:
            store.state.theme === "dark"
              ? "rgba(0,0,0,1)"
              : "rgba(255,255,255,1)",
          extraCssText:
            "max-height: 200px; overflow: auto; max-width: 500px; user-select: text;",
        };

        // Set coordinate system options
        options.xAxis = [{ type: "value", show: false }];
        options.yAxis = [{ type: "value", show: false }];
        options.angleAxis = {
          show: false,
        };
        options.radiusAxis = {
          show: false,
        };
        options.polar = {};
        return series;
      }
      case "metric": {
        isTimeSeriesFlag = false;

        switch (it?.resultType) {
          case "matrix": {
            // take first result
            const metric = it?.result?.[0];

            const values = metric.values.sort((a: any, b: any) => a[0] - b[0]);
            const latestValue = values[values.length - 1]?.[1] ?? 0;

            const unitValue = getUnitValue(
              latestValue,
              panelSchema.config?.unit,
              panelSchema.config?.unit_custom,
              panelSchema.config?.decimals,
            );
            options.backgroundColor =
              panelSchema.config?.background?.value?.color ?? "";
            const series = [
              {
                type: "custom",
                silent: true,
                coordinateSystem: "polar",
                renderItem: function (params: any) {
                  const backgroundColor =
                    panelSchema.config?.background?.value?.color;
                  const isDarkTheme = store.state.theme === "dark";
                  return {
                    type: "text",
                    style: {
                      text: formatUnitValue(unitValue),
                      fontSize: calculateOptimalFontSize(
                        formatUnitValue(unitValue),
                        params.coordSys.cx * 2,
                      ), //coordSys is relative. so that we can use it to calculate the dynamic size
                      fontWeight: 500,
                      align: "center",
                      verticalAlign: "middle",
                      x: params.coordSys.cx,
                      y: params.coordSys.cy,
                      fill: getContrastColor(backgroundColor, isDarkTheme),
                    },
                  };
                },
              },
            ];

            options.dataset = { source: [[]] };
            options.tooltip = {
              show: false,
            };
            options.angleAxis = {
              show: false,
            };
            options.radiusAxis = {
              show: false,
            };
            options.polar = {};
            options.xAxis = [];
            options.yAxis = [];

            return series;
          }

          case "vector": {
            const traces = it?.result?.map((metric: any) => {
              const values = [metric.value];
              return {
                name: JSON.stringify(metric.metric),
                value: metric?.value?.length > 1 ? metric.value[1] : "",
                ...getPropsByChartTypeForSeries(panelSchema.type),
              };
            });
            return traces;
          }
        }
        break;
      }
      default: {
        return [];
      }
    }
  });

  const convertedTimeStampToDataFormat = new Date(
    annotations?.value?.[0]?.start_time / 1000,
  ).toString();

  // mark line and mark area will be added only for time series chart
  // with specific chart type
  if (
    [
      "area",
      "area-stacked",
      "bar",
      "h-bar",
      "line",
      "scatter",
      "stacked",
      "h-stacked",
    ].includes(panelSchema.type) &&
    isTimeSeriesFlag &&
    !panelSchema.config.trellis?.layout
  ) {
    options.series.push({
      type: "line",
      data: [[convertedTimeStampToDataFormat, null]],
      markLine: {
        itemStyle: {
          color: "rgba(108, 122, 125, 1)",
        },
        silent: false,
        animation: false,
        data: markLines,
      },
      markArea: getSeriesMarkArea(),
      zlevel: 1,
    });
  }

  options.series = options.series.flat();

  //from this maxValue want to set the width of the chart based on max value is greater than 30% than give default legend width other wise based on max value get legend width
  //only check for vertical side only
  if (
    legendConfig.orient == "vertical" &&
    panelSchema.config?.show_legends &&
    panelSchema.type != "gauge" &&
    panelSchema.type != "metric"
  ) {
    let legendWidth;

    if (
      panelSchema.config.legend_width &&
      !isNaN(parseFloat(panelSchema.config.legend_width.value))
      // ["px", "%"].includes(panelSchema.config.legend_width.unit)
    ) {
      if (panelSchema.config.legend_width.unit === "%") {
        // If in percentage, calculate percentage of the chartPanelRef width
        const percentage = panelSchema.config.legend_width.value / 100;
        legendWidth = chartPanelRef.value?.offsetWidth * percentage;
      } else {
        // If in pixels, use the provided value
        legendWidth = panelSchema.config.legend_width.value;
      }
    } else {
      const maxValue = options.series.reduce((max: any, it: any) => {
        return max.length < it?.name?.length ? it?.name : max;
      }, "");

      // If legend_width is not provided or has invalid format, calculate it based on other criteria
      legendWidth =
        Math.min(
          chartPanelRef.value?.offsetWidth / 3,
          calculateWidthText(maxValue) + 60,
        ) ?? 20;
    }

    options.grid.right = legendWidth;
    options.legend.textStyle.width = legendWidth - 55;
  }

  //check if is there any data else filter out axis or series data
  if (!options?.series?.length && !options?.xAxis?.length) {
    // console.timeEnd("convertPromQLData");
    return {
      options: {
        series: [],
        xAxis: [],
      },
    };
  }

  moment = null;

  // allowed to zoom, only if timeseries
  options.toolbox.show = options.toolbox.show && isTimeSeriesFlag;
  // promql query will be always timeseries except gauge and metric text chart.
  // console.timeEnd("convertPromQLData");
  return {
    options,
    extras: {
      ...extras,
      panelId: panelSchema?.id,
      isTimeSeries: isTimeSeriesFlag,
    },
  };
};

const calculateWidthText = (text: string): number => {
  if (!text) return 0;

  const span = document.createElement("span");
  document.body.appendChild(span);

  span.style.font = "sans-serif";
  span.style.fontSize = "12px";
  span.style.height = "auto";
  span.style.width = "auto";
  span.style.top = "0px";
  span.style.position = "absolute";
  span.style.whiteSpace = "no-wrap";
  span.innerHTML = text;

  const width = Math.ceil(span.clientWidth);
  span.remove();
  return width;
};

/**
 * Retrieves the legend name for a given metric and label.
 *
 * @param {any} metric - The metric object containing the values for the legend name placeholders.
 * @param {string} label - The label template for the legend name. If null or empty, the metric object will be converted to a JSON string and returned.
 * @return {string} The legend name with the placeholders replaced by the corresponding values from the metric object.
 */
const getPromqlLegendName = (metric: any, label: string) => {
  if (label) {
    let template = label || "";
    const placeholders = template.match(/\{([^}]+)\}/g);

    // Step 2: Iterate through each placeholder
    placeholders?.forEach(function (placeholder: any) {
      // Step 3: Extract the key from the placeholder
      const key = placeholder.replace("{", "").replace("}", "");

      // Step 4: Retrieve the corresponding value from the JSON object
      const value = metric[key];

      // Step 5: Replace the placeholder with the value in the template
      if (value) {
        template = template.replace(placeholder, value);
      }
    });
    return template;
  } else {
    return JSON.stringify(metric);
  }
};

/**
 * Determines the position of the legend based on the provided legendPosition.
 *
 * @param {string} legendPosition - The desired position of the legend. Possible values are "bottom" or "right".
 * @return {string} The position of the legend. Possible values are "horizontal" or "vertical".
 */
const getLegendPosition = (legendPosition: string) => {
  switch (legendPosition) {
    case "bottom":
      return "horizontal";
    case "right":
      return "vertical";
    default:
      return "horizontal";
  }
};

/**
 * Returns the props object based on the given chart type.
 *
 * @param {string} type - The chart type.
 * @return {object} The props object for the given chart type.
 */
const getPropsByChartTypeForSeries = (type: string) => {
  switch (type) {
    case "bar":
      return {
        type: "bar",
        emphasis: { focus: "series" },
        lineStyle: { width: 1.5 },
      };
    case "line":
      return {
        type: "line",
        emphasis: { focus: "series" },
        lineStyle: { width: 1.5 },
      };
    case "scatter":
      return {
        type: "scatter",
        emphasis: { focus: "series" },
        symbolSize: 5,
      };
    case "pie":
      return {
        type: "pie",
        emphasis: { focus: "series" },
        lineStyle: { width: 1.5 },
      };
    case "donut":
      return {
        type: "pie",
        emphasis: { focus: "series" },
        lineStyle: { width: 1.5 },
      };
    case "h-bar":
      return {
        type: "bar",
        orientation: "h",
        emphasis: { focus: "series" },
        lineStyle: { width: 1.5 },
      };
    case "area":
      return {
        type: "line",
        emphasis: { focus: "series" },
        areaStyle: {},
        lineStyle: { width: 1.5 },
      };
    case "stacked":
      return {
        type: "bar",
        emphasis: { focus: "series" },
        lineStyle: { width: 1.5 },
      };
    case "area-stacked":
      return {
        type: "line",
        stack: "Total",
        areaStyle: {},
        emphasis: {
          focus: "series",
        },
        lineStyle: { width: 1.5 },
      };
    case "gauge":
      return {
        type: "gauge",
        startAngle: 205,
        endAngle: -25,
        pointer: {
          show: false,
        },
        axisTick: {
          show: false,
        },
        splitLine: {
          show: false,
        },
        axisLabel: {
          show: false,
        },
      };
    case "metric":
      return {
        type: "custom",
        coordinateSystem: "polar",
      };
    case "h-stacked":
      return {
        type: "bar",
        emphasis: { focus: "series" },
        orientation: "h",
        lineStyle: { width: 1.5 },
      };
    default:
      return {
        type: "bar",
      };
  }
};
