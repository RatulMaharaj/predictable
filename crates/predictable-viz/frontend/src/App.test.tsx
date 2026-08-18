// Component tests for Screen 1. They drive the real `App` over a real
// `GraphDoc` produced by `predictable graph --json`, with only the layout worker
// swapped for a deterministic grid — so what is asserted is the screen's
// behaviour, not a mock's.

import { beforeEach, describe, expect, it } from "vitest";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { App, shortDigest } from "./App";
import { Inspector, tokenizeExpr } from "./Inspector";
import { Palette } from "./Palette";
import { LayoutCache } from "./graph/layout";
import type { Layout } from "./graph/layout";
import type { LayoutEngine } from "./graph/useLayout";
import { MemoryStore, StubDataSource, TERM_ANNUAL, withProphet } from "./test/fixtures";
import { INITIAL_VIEW, useView } from "./store";
import type { GraphDoc } from "./datasource";

function gridEngine(doc: GraphDoc): LayoutEngine {
  return {
    async layout() {
      const perDepth = new Map<number, number>();
      const nodes: Layout["nodes"] = {};
      for (const node of doc.nodes) {
        const row = perDepth.get(node.depth) ?? 0;
        perDepth.set(node.depth, row + 1);
        nodes[node.id] = { x: node.depth * 200, y: row * 50, width: 120, height: 34 };
      }
      return { model_digest: doc.model_digest, width: 2000, height: 1600, nodes, edges: {} };
    },
  };
}

async function mount(doc: GraphDoc = TERM_ANNUAL) {
  const view = render(
    <App
      source={new StubDataSource(doc)}
      engine={gridEngine(doc)}
      cache={new LayoutCache(new MemoryStore())}
    />,
  );
  await screen.findByTestId("graph-svg");
  return view;
}

beforeEach(() => {
  // zustand merges, so the optional keys must be cleared by name.
  useView.setState({ ...INITIAL_VIEW, selected: undefined, modelpoint: undefined, t: undefined });
  history.replaceState(null, "", "/");
});

describe("the Model Explorer draws the plan it was sent", () => {
  it("renders a node per component and shows the digests", async () => {
    await mount();
    expect(screen.getAllByTestId("graph-node").length).toBe(TERM_ANNUAL.nodes.length);
    expect(shortDigest(TERM_ANNUAL.model_digest)).toBe("46e1065");
    expect(screen.getByText("model 46e1065")).toBeInTheDocument();
    expect(screen.getByTitle(TERM_ANNUAL.model_digest)).toBeInTheDocument();
    expect(screen.getByTestId("counts")).toHaveTextContent(
      `${TERM_ANNUAL.nodes.length} nodes`,
    );
    expect(screen.getByTestId("renderer")).toHaveTextContent("svg");
  });

  it("draws the `t-1` self-loop for a recursive component", async () => {
    await mount();
    const loops = screen.getAllByTestId("self-loop").map((el) => el.dataset.id);
    expect(loops).toContain("model.num_pols_if");
    expect(screen.getAllByText("t-1").length).toBeGreaterThan(0);
  });

  it("shows the empty state, not an error, with no run loaded", async () => {
    await mount();
    await userEvent.click(screen.getAllByTestId("graph-node").find((n) => n.dataset.id === "model.qx")!);
    expect(await screen.findByTestId("no-run")).toHaveTextContent("load a run to see values");
  });
});

describe("selection and the inspector", () => {
  it("clicking a node opens its inspector and writes the selection to the URL", async () => {
    await mount();
    const node = screen.getAllByTestId("graph-node").find((n) => n.dataset.id === "model.num_pols_if")!;
    await userEvent.click(node);
    const inspector = await screen.findByTestId("inspector");
    expect(inspector.dataset.id).toBe("model.num_pols_if");
    expect(within(inspector).getByRole("heading", { level: 2 })).toHaveTextContent("num_pols_if");
    expect(within(inspector).getByTestId("expr")).toHaveTextContent("num_pols_if[t-1]");
    expect(location.search).toContain("sel=model.num_pols_if");
  });

  it("every identifier in the expression navigates to its own node", async () => {
    await mount();
    await userEvent.click(
      screen.getAllByTestId("graph-node").find((n) => n.dataset.id === "model.num_pols_if")!,
    );
    const links = (await screen.findAllByTestId("expr-link")).map((el) => el.dataset.name);
    expect(links).toContain("qx");
    const qx = (await screen.findAllByTestId("expr-link")).find((el) => el.dataset.name === "qx")!;
    await userEvent.click(qx);
    await waitFor(() => expect(screen.getByTestId("inspector").dataset.id).toBe("model.qx"));
  });

  it("lists the planner's upstream and downstream, not a local recomputation", async () => {
    await mount();
    await userEvent.click(
      screen.getAllByTestId("graph-node").find((n) => n.dataset.id === "model.deaths")!,
    );
    const inspector = await screen.findByTestId("inspector");
    const upstream = within(inspector).getAllByTestId("upstream").map((b) => b.textContent);
    expect(upstream.join(" ")).toContain("model.num_pols_if");
  });
});

describe("the impact overlay (§2.3)", () => {
  it("`I` highlights the transitive downstream closure and counts it", async () => {
    await mount();
    await userEvent.click(screen.getAllByTestId("graph-node").find((n) => n.dataset.id === "model.qx")!);
    await userEvent.keyboard("i");
    const count = await screen.findByTestId("impact-count");
    expect(count.textContent).toMatch(/^\d+ impacted, \d+ Outputs$/);
    expect(await screen.findByTestId("impact-summary")).toHaveTextContent(
      /changing qx affects \d+ components, \d+ of them Outputs/,
    );
    expect(location.search).toContain("impact=1");
  });

  it("does not fire `I` while the palette input has focus", async () => {
    await mount();
    await userEvent.click(screen.getAllByTestId("graph-node").find((n) => n.dataset.id === "model.qx")!);
    await userEvent.keyboard("{Meta>}k{/Meta}");
    await userEvent.keyboard("i");
    expect(screen.queryByTestId("impact-count")).not.toBeInTheDocument();
    expect((screen.getByLabelText("search components") as HTMLInputElement).value).toBe("i");
  });
});

describe("filters", () => {
  it("filtering by kind removes the other nodes from the picture", async () => {
    await mount();
    await userEvent.click(
      within(screen.getByTestId("facet-kind")).getByLabelText("Output"),
    );
    await waitFor(() =>
      expect(screen.getAllByTestId("graph-node").length).toBeLessThan(TERM_ANNUAL.nodes.length),
    );
    expect(
      screen.getAllByTestId("graph-node").every((n) => n.dataset.kind === "Output"),
    ).toBe(true);
    expect(location.search).toContain("kind=Output");
  });

  it("layer bands are off by default and appear when toggled", async () => {
    await mount();
    expect(document.querySelectorAll(".band").length).toBe(0);
    await userEvent.click(screen.getByLabelText(/layers/i));
    await waitFor(() => expect(document.querySelectorAll(".band").length).toBe(9));
  });
});

describe("the ⌘K palette", () => {
  it("opens on Cmd-K and selects the hit on Enter", async () => {
    await mount();
    await userEvent.keyboard("{Meta>}k{/Meta}");
    const input = await screen.findByLabelText("search components");
    await userEvent.type(input, "num_pols");
    expect((await screen.findAllByTestId("palette-hit"))[0].dataset.id).toBe("model.num_pols_if");
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(screen.getByTestId("inspector").dataset.id).toBe("model.num_pols_if"));
    expect(screen.queryByTestId("palette")).not.toBeInTheDocument();
  });

  it("finds a component by its Prophet variable name and says that is why", async () => {
    // `BEL_TOT` is the migration case that matters: the predictable component is
    // called `reserve` and shares not one character with the Prophet variable.
    await mount(withProphet());
    await userEvent.keyboard("{Meta>}k{/Meta}");
    await userEvent.type(await screen.findByLabelText("search components"), "BEL_TOT");
    const first = (await screen.findAllByTestId("palette-hit"))[0];
    expect(first.dataset.id).toBe("model.reserve");
    expect(first.dataset.field).toBe("prophet");
    expect(first).toHaveTextContent("Prophet TERM_UK.BEL_TOT");
    await userEvent.keyboard("{Enter}");
    await waitFor(() => expect(screen.getByTestId("inspector").dataset.id).toBe("model.reserve"));
    expect(screen.getByTestId("provenance")).toHaveTextContent("TERM_UK.BEL_TOT");
  });

  it("closes on Escape without changing the selection", async () => {
    await mount();
    await userEvent.keyboard("{Meta>}k{/Meta}");
    await screen.findByTestId("palette");
    await userEvent.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByTestId("palette")).not.toBeInTheDocument());
    expect(screen.getByTestId("inspector")).toHaveTextContent("Select a component");
  });
});

describe("Palette, standalone", () => {
  it("says so when nothing matches", async () => {
    render(
      <Palette doc={TERM_ANNUAL} open onClose={() => {}} onPick={() => {}} />,
    );
    await userEvent.type(screen.getByLabelText("search components"), "zzzzz");
    expect(await screen.findByText("no component matches")).toBeInTheDocument();
  });

  it("arrow keys move the cursor", async () => {
    render(<Palette doc={TERM_ANNUAL} open onClose={() => {}} onPick={() => {}} />);
    await userEvent.type(screen.getByLabelText("search components"), "e");
    const before = screen.getAllByTestId("palette-hit")[0].getAttribute("aria-selected");
    expect(before).toBe("true");
    await userEvent.keyboard("{ArrowDown}");
    expect(screen.getAllByTestId("palette-hit")[1]).toHaveAttribute("aria-selected", "true");
    await userEvent.keyboard("{ArrowUp}{ArrowUp}");
    expect(screen.getAllByTestId("palette-hit")[0]).toHaveAttribute("aria-selected", "true");
  });
});

describe("Inspector, standalone", () => {
  it("splits an expression into identifiers and the rest", () => {
    expect(tokenizeExpr("a[t-1] * (1 - qx)").filter((t) => t.identifier).map((t) => t.text)).toEqual([
      "a",
      "t",
      "qx",
    ]);
    expect(tokenizeExpr("1.0").every((t) => !t.identifier)).toBe(true);
  });

  it("shows Prophet provenance when the component claims a variable", () => {
    const doc = withProphet();
    const node = doc.nodes.find((n) => n.id === "model.num_pols_if")!;
    render(
      <Inspector
        doc={{ node, upstream: [], downstream: [] }}
        hasRun={false}
        onNavigate={() => {}}
        resolves={() => false}
      />,
    );
    const provenance = screen.getByTestId("provenance");
    expect(provenance).toHaveTextContent("TERM_UK.NUM_POLS_IF");
    expect(provenance).toHaveTextContent("TERM.VAR:42");
  });

  it("shows lints in the panel as well as on the node", () => {
    const node = withProphet().nodes.find((n) => n.id === "model.expense_scale")!;
    render(
      <Inspector
        doc={{ node, upstream: [], downstream: [] }}
        hasRun={false}
        onNavigate={() => {}}
        resolves={() => false}
      />,
    );
    expect(screen.getByTestId("lints")).toHaveTextContent("W0104");
  });

  it("prompts for a selection rather than rendering blank", () => {
    render(<Inspector doc={null} hasRun={false} onNavigate={() => {}} resolves={() => false} />);
    expect(screen.getByTestId("inspector")).toHaveTextContent("Select a component to inspect it.");
  });
});
