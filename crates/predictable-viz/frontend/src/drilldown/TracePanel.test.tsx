import { describe, expect, it, vi } from "vitest";
import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { TracePanel } from "./TracePanel";
import reserveTrace from "./__fixtures__/trace-reserve.json";
import aggTrace from "./__fixtures__/trace-agg.json";
import { flattenTrace, type Trace, type TraceNode } from "./trace";

const reserve = reserveTrace as unknown as Trace;
const agg = aggTrace as unknown as Trace;

function nodes(node: TraceNode): TraceNode[] {
  return [node, ...(node.children ?? []).flatMap(nodes)];
}

const noop = () => {};

describe("TracePanel renders the engine's tree verbatim", () => {
  it("emits one row per node, in the engine's order, keyed by its path", () => {
    render(<TracePanel trace={reserve} collapsed={new Set()} onToggle={noop} />);
    const rows = [...document.querySelectorAll(".trace-row")];
    const expected = flattenTrace(reserve.root).map((r) => r.node.path);
    expect(rows.map((r) => r.getAttribute("data-path"))).toEqual(expected);
  });

  it("shows the root's own header: component, modelpoint, t, value, unit and expr", () => {
    render(<TracePanel trace={reserve} collapsed={new Set()} onToggle={noop} />);
    const head = document.querySelector(".trace-head")!;
    expect(head.textContent).toContain("model.reserve");
    expect(head.textContent).toContain("TA00001");
    expect(head.textContent).toContain("t = 3");
    expect(head.textContent).toContain(reserve.root.unit!);
    expect(document.querySelector(".trace-expr")!.textContent).toBe(reserve.root.expr);
  });

  it("carries the exact f64 of every node on hover", () => {
    render(<TracePanel trace={reserve} collapsed={new Set()} onToggle={noop} />);
    const titles = [...document.querySelectorAll(".trace-num")].map((n) => n.getAttribute("title"));
    const values = flattenTrace(reserve.root).map((r) => String(r.node.value));
    expect(titles).toEqual(values);
  });

  it("attaches each note to its own node, tinting that row", () => {
    render(<TracePanel trace={reserve} collapsed={new Set()} onToggle={noop} />);
    for (const note of reserve.notes!) {
      const row = document.querySelector(`[data-path="${CSS.escape(note.path)}"]`)!;
      expect(row.className).toContain("is-warn");
      expect(row.textContent).toContain(note.code);
      expect(row.textContent).toContain(note.message);
    }
  });

  it("folds and unfolds a subtree, reporting it to the URL owner", async () => {
    const user = userEvent.setup();
    const onToggle = vi.fn();
    const branch = reserve.root.children![0];
    const { rerender } = render(
      <TracePanel trace={reserve} collapsed={new Set()} onToggle={onToggle} />,
    );
    const before = document.querySelectorAll(".trace-row").length;
    await user.click(screen.getByRole("button", { name: `collapse ${branch.path}` }));
    expect(onToggle).toHaveBeenCalledWith(branch.path);

    rerender(<TracePanel trace={reserve} collapsed={new Set([branch.path])} onToggle={onToggle} />);
    const after = document.querySelectorAll(".trace-row").length;
    expect(after).toBe(before - (nodes(branch).length - 1));
    expect(screen.getByRole("button", { name: `expand ${branch.path}` })).toBeDefined();
  });

  it("navigates from a Ref to that component's own trace at that t", async () => {
    const user = userEvent.setup();
    const onNavigate = vi.fn();
    render(
      <TracePanel trace={reserve} collapsed={new Set()} onToggle={noop} onNavigate={onNavigate} />,
    );
    const ref = nodes(reserve.root).find((n) => n.node === "Lag")!;
    const row = document.querySelector(`[data-path="${CSS.escape(ref.path)}"]`)!;
    await user.click(within(row as HTMLElement).getByRole("button", { name: /\[t-/ }));
    expect(onNavigate).toHaveBeenCalledWith(ref.ref, ref.t);
  });

  it("opens the .pir at the span the node carries", async () => {
    // The term_annual run was built without a span map, so its trace carries no
    // spans: the affordance is exercised on a node that does have one.
    const user = userEvent.setup();
    const onOpenSpan = vi.fn();
    const span = {
      file: "models/term_annual/model.pir",
      line: 148,
      col: 9,
      byte_start: 4021,
      byte_end: 4123,
    };
    const node: TraceNode = { node: "Lit", path: "root.children[0]", value: 1, span };
    const trace: Trace = {
      format: "pvf/1",
      kind: "trace",
      run: "",
      notes: [],
      truncated: { depth: 0, elided_nodes: 0 },
      root: { node: "Component", path: "root", value: 1, id: "m.x", t: 0, children: [node] },
    };
    render(<TracePanel trace={trace} collapsed={new Set()} onToggle={noop} onOpenSpan={onOpenSpan} />);
    await user.click(screen.getByText("models/term_annual/model.pir:148"));
    expect(onOpenSpan).toHaveBeenCalledWith(node);
  });

  it("opens the table viewer at the resolved row of a Lookup", async () => {
    const user = userEvent.setup();
    const onOpenTable = vi.fn();
    render(
      <TracePanel trace={reserve} collapsed={new Set()} onToggle={noop} onOpenTable={onOpenTable} />,
    );
    const lookup = nodes(reserve.root).find((n) => n.node === "Lookup")!;
    await user.click(screen.getByText(`open table at row ${lookup.row}`));
    expect(onOpenTable).toHaveBeenCalledWith(lookup.table, lookup.row);
  });

  it("highlights the matching waterfall bar while a row is hovered", async () => {
    const user = userEvent.setup();
    const onHighlight = vi.fn();
    render(
      <TracePanel trace={reserve} collapsed={new Set()} onToggle={noop} onHighlight={onHighlight} />,
    );
    const ref = nodes(reserve.root).find((n) => n.node === "Ref")!;
    await user.hover(document.querySelector(`[data-path="${CSS.escape(ref.path)}"]`)!);
    expect(onHighlight).toHaveBeenCalledWith(ref.ref);
  });

  it("says how much it truncated instead of pretending the tree is whole", () => {
    render(<TracePanel trace={reserve} collapsed={new Set()} onToggle={noop} />);
    expect(document.querySelector(".trace-truncated")!.textContent).toContain(
      `${reserve.truncated.elided_nodes} nodes elided`,
    );
    expect(screen.getAllByText(/children elided/).length).toBeGreaterThan(0);
  });

  it("renders the per-t contributions of an Agg, disc factor included", () => {
    render(<TracePanel trace={agg} collapsed={new Set()} onToggle={noop} />);
    const table = document.querySelector(".trace-terms")!;
    const body = table.querySelectorAll("tbody tr");
    const node = nodes(agg.root).find((n) => n.node === "Agg")!;
    expect(body).toHaveLength(node.terms!.length);
    expect(body[0].textContent).toContain(String(node.terms![0].t));
    expect(table.querySelectorAll("thead th")[2].textContent).toBe("disc");
  });

  it("badges a pre-origin read so the off-by-one at t = 0 is visible", () => {
    const trace: Trace = {
      format: "pvf/1",
      kind: "trace",
      run: "",
      truncated: { depth: 0, elided_nodes: 0 },
      notes: [],
      root: {
        node: "Component",
        path: "root",
        value: 1084.2,
        id: "model.reserve",
        t: 0,
        children: [
          {
            node: "Lag",
            path: "root.children[0]",
            value: 842.1,
            ref: "model.reserve",
            lag: 1,
            t: -1,
            resolution: "init",
            init_expr: "bel",
          },
        ],
      },
    };
    render(<TracePanel trace={trace} collapsed={new Set()} onToggle={noop} />);
    const badge = document.querySelector(".trace-badge.is-warn")!;
    expect(badge.textContent).toBe("t=-1 before origin → used init = bel");
    expect(document.querySelector('[data-path="root.children[0]"]')!.className).toContain("is-warn");
  });
});
