import { describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { tableFromArrays, type Table } from "apache-arrow";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { DrilldownScreen } from "./DrilldownScreen";
import { DEFAULT_ROUTE, type DrilldownRoute } from "./route";
import { ApiError } from "../datasource";
import type { DataSource, GraphDoc, GraphNode, SeriesQuery } from "../datasource";
import reserveTrace from "./__fixtures__/trace-reserve.json";

const T = [0, 1, 2, 3];

const series: Record<string, number[]> = {
  "model.premium_income": [1240, 1265.7, 1291.4, 1317.3],
  "model.death_claims": [0, 134.2, 138.9, 143.7],
  "model.reserve": [1084.2, 1131.8, 1180.1, 1284.5512345],
};

function node(id: string, i: number, extra: Partial<GraphNode> = {}): GraphNode {
  return {
    id,
    name: id.split(".")[1],
    module: "model",
    kind: "Derived",
    dtype: "f64",
    shape: "Series",
    unit: "money",
    stage: 1,
    tags: [],
    declaration_index: i,
    depth: i,
    ...extra,
  };
}

const graph: GraphDoc = {
  ir_version: "1.0",
  model_digest: "sha256:aa",
  plan_digest: "sha256:bb",
  order_digest: "sha256:cc",
  modules: ["model"],
  nodes: [
    // deliberately out of declaration order, and with one non-Series component
    node("model.reserve", 2, { timing: "end", kind: "Output" }),
    node("model.premium_income", 0, { timing: "start" }),
    node("model.death_claims", 1, { timing: "end" }),
    node("model.bel", 9, { shape: "PerMP" }),
  ],
  edges: [],
  layers: [],
};

class FakeSource implements DataSource {
  seriesCalls: SeriesQuery[] = [];
  explainCalls: unknown[] = [];
  constructor(private readonly explainResult: unknown = reserveTrace) {}
  manifest = async () => ({});
  graph = async () => graph;
  component = async () => ({
    node: graph.nodes[0],
    upstream: [],
    downstream: [],
  });
  capabilities = async () => ({
    explain: true,
    recompute: false,
    sensitivity: false,
  });
  diff = async (): Promise<never> => {
    throw new ApiError(501, "unsupported", "`diff` is not available from this data source");
  };
  async series(q: SeriesQuery): Promise<Table> {
    this.seriesCalls.push(q);
    const columns: Record<string, unknown> = {
      modelpoint: T.map(() => "TA00001"),
      t: Uint32Array.from(T),
    };
    for (const name of q.components) {
      if (series[name]) columns[name] = Float64Array.from(series[name]);
    }
    return tableFromArrays(columns as never) as unknown as Table;
  }
  async explain(q: unknown) {
    this.explainCalls.push(q);
    if (this.explainResult instanceof Error) throw this.explainResult;
    return this.explainResult;
  }
}

function Harness({ source, initial }: { source: DataSource; initial: DrilldownRoute }) {
  const [route, setRoute] = useState(initial);
  return (
    <DrilldownScreen source={source} route={route} onRouteChange={setRoute} tFrom={0} tTo={3} />
  );
}

const mpRoute: DrilldownRoute = {
  ...DEFAULT_ROUTE,
  view: "mp",
  run: "runs/base",
  group: "product=TERM",
  mp: "TA00001",
};

describe("DrilldownScreen", () => {
  it("asks the engine for the Series components in declaration order", async () => {
    const source = new FakeSource();
    render(<Harness source={source} initial={mpRoute} />);
    await screen.findByRole("grid");
    expect(source.seriesCalls[0]).toEqual({
      run: "runs/base",
      components: ["model.premium_income", "model.death_claims", "model.reserve"],
      modelpoints: ["TA00001"],
      tFrom: 0,
      tTo: 3,
    });
  });

  it("shows the breadcrumb from the URL", async () => {
    render(<Harness source={new FakeSource()} initial={mpRoute} />);
    await screen.findByRole("grid");
    expect(screen.getByRole("navigation").textContent).toBe("portfolioproduct=TERMTA00001");
  });

  it("does not call explain() until a cell is chosen", async () => {
    const source = new FakeSource();
    render(<Harness source={source} initial={mpRoute} />);
    await screen.findByRole("grid");
    expect(source.explainCalls).toEqual([]);
    expect(screen.getByText("select a cell to explain it")).toBeDefined();
  });

  it("explains the cell the reader clicks, and puts it in the route", async () => {
    const user = userEvent.setup();
    const source = new FakeSource();
    render(<Harness source={source} initial={mpRoute} />);
    await screen.findByRole("grid");
    await user.dblClick(screen.getByText("1,284.5512"));
    await waitFor(() => expect(source.explainCalls).toHaveLength(1));
    expect(source.explainCalls[0]).toEqual({
      run: "runs/base",
      component: "model.reserve",
      modelpoint: "TA00001",
      t: 3,
    });
    expect(await screen.findByText(/^explain/)).toBeDefined();
    expect(document.querySelectorAll(".trace-row").length).toBeGreaterThan(10);
  });

  it("builds the waterfall from the selected period, closing on the selected component", async () => {
    const user = userEvent.setup();
    render(<Harness source={new FakeSource()} initial={mpRoute} />);
    await screen.findByRole("grid");
    await user.dblClick(screen.getByText("1,284.5512"));
    const bars = [...document.querySelectorAll(".wf-bar")];
    expect(bars.map((b) => b.getAttribute("data-component"))).toEqual([
      "model.premium_income",
      "model.death_claims",
      "model.reserve",
    ]);
    expect(bars[2].getAttribute("class")!).toContain("is-output");
    expect(screen.getByRole("region", { name: "cashflow waterfall at t = 3" })).toBeDefined();
  });

  it("moves the trace when a waterfall bar is clicked — same data, different zoom", async () => {
    const user = userEvent.setup();
    const source = new FakeSource();
    render(<Harness source={source} initial={{ ...mpRoute, component: "model.reserve", t: 3 }} />);
    await screen.findByRole("grid");
    await waitFor(() => expect(source.explainCalls).toHaveLength(1));
    const chart = screen.getByRole("region", { name: /cashflow waterfall/ });
    await user.click(within(chart).getByRole("button", { name: /model.death_claims/ }));
    await waitFor(() => expect(source.explainCalls).toHaveLength(2));
    expect(source.explainCalls[1]).toMatchObject({
      component: "model.death_claims",
      t: 3,
    });
  });

  it("keeps the grid selection and the route in step", async () => {
    const user = userEvent.setup();
    render(
      <Harness
        source={new FakeSource()}
        initial={{ ...mpRoute, component: "model.reserve", t: 1 }}
      />,
    );
    await screen.findByRole("grid");
    expect(document.querySelector(".mp-grid-cell.is-focus")!.textContent).toBe("1,131.8");
    screen.getByRole("grid").focus();
    await user.keyboard("{ArrowUp}");
    await waitFor(() =>
      expect(document.querySelector(".mp-grid-cell.is-focus")!.textContent).toBe("134.2"),
    );
  });

  it("toggles the heat overlay through the route", async () => {
    const user = userEvent.setup();
    render(<Harness source={new FakeSource()} initial={mpRoute} />);
    await screen.findByRole("grid");
    expect(document.querySelector("[data-heat]")).toBeNull();
    await user.click(screen.getByLabelText("heat"));
    await waitFor(() => expect(document.querySelector("[data-heat]")).not.toBeNull());
  });

  it("says explain() is unavailable rather than showing an empty trace", async () => {
    const source = new FakeSource(new Error("explain is not enabled for this run"));
    render(<Harness source={source} initial={{ ...mpRoute, component: "model.reserve", t: 3 }} />);
    expect(
      await screen.findByText("explain() unavailable: explain is not enabled for this run"),
    ).toBeDefined();
  });

  it("surfaces a dead run instead of rendering half a screen", async () => {
    const source = new FakeSource();
    source.graph = vi.fn().mockRejectedValue(new Error("connection refused"));
    render(<Harness source={source} initial={mpRoute} />);
    expect(await screen.findByText("cannot reach the run: connection refused")).toBeDefined();
  });
});
