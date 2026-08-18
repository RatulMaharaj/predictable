import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Waterfall } from "./WaterfallChart";
import type { Flow } from "./waterfall";

const flows: Flow[] = [
  { component: "premium_income", value: 1317.3, timing: "start", declarationIndex: 0 },
  { component: "death_claims", value: -143.7, timing: "end", declarationIndex: 1 },
  { component: "renewal_expenses", value: -48.2, timing: "start", declarationIndex: 2 },
  { component: "reserve", value: 1284.55, timing: "point", kind: "Output", declarationIndex: 9 },
];

const bars = () => [...document.querySelectorAll(".wf-bar")];

describe("Waterfall", () => {
  it("draws one bar per flow, in declaration order", () => {
    render(<Waterfall flows={flows} t={3} />);
    expect(bars().map((b) => b.getAttribute("data-component"))).toEqual([
      "premium_income",
      "death_claims",
      "renewal_expenses",
      "reserve",
    ]);
  });

  it("shows every bar's timing as a glyph and as a word", () => {
    render(<Waterfall flows={flows} t={3} />);
    expect(screen.getAllByText("start")).toHaveLength(2);
    expect(screen.getByText("end")).toBeDefined();
    expect(document.querySelectorAll(".wf-glyph")).toHaveLength(4);
  });

  it("warns when timings are mixed in one sum", () => {
    render(<Waterfall flows={flows} t={3} />);
    expect(screen.getByRole("note").textContent).toContain("different timings are summed");
  });

  it("signs negative bars in class as well as in colour", () => {
    render(<Waterfall flows={flows} t={3} />);
    expect(bars()[1].getAttribute("class")!).toContain("is-negative");
    expect(bars()[0].getAttribute("class")!).toContain("is-positive");
    expect(bars()[3].getAttribute("class")!).toContain("is-output");
  });

  it("puts the exact value, timing and running total in each bar's tooltip", () => {
    render(<Waterfall flows={flows} t={3} />);
    expect(bars()[1].querySelector("title")!.textContent).toBe(
      "death_claims -143.7 · timing end · running 1173.6",
    );
  });

  it("opens explain() for the clicked bar", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(<Waterfall flows={flows} t={3} onSelect={onSelect} />);
    await user.click(screen.getByRole("button", { name: /death_claims/ }));
    expect(onSelect).toHaveBeenCalledWith("death_claims");
  });

  it("is reachable from the keyboard", async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(<Waterfall flows={flows} t={3} onSelect={onSelect} />);
    await user.tab(); // the sort toggle
    await user.tab(); // the first bar
    await user.keyboard("{Enter}");
    expect(onSelect).toHaveBeenCalledWith("premium_income");
  });

  it("re-orders on the magnitude toggle without changing the total", async () => {
    const user = userEvent.setup();
    const onToggleSort = vi.fn();
    const { rerender } = render(<Waterfall flows={flows} t={3} onToggleSort={onToggleSort} />);
    await user.click(screen.getByLabelText("sort by magnitude"));
    expect(onToggleSort).toHaveBeenCalled();
    rerender(<Waterfall flows={flows} t={3} sortByMagnitude onToggleSort={onToggleSort} />);
    expect(bars().map((b) => b.getAttribute("data-component"))).toEqual([
      "premium_income",
      "death_claims",
      "renewal_expenses",
      "reserve",
    ]);
    expect(document.querySelector(".wf-total")!.textContent).toContain("+1,125.4");
  });

  it("offers the chart as a table, with unrounded numbers", () => {
    render(<Waterfall flows={flows} t={3} showTable />);
    const rows = document.querySelectorAll(".wf-table tbody tr");
    expect(rows).toHaveLength(4);
    expect(rows[0].textContent).toBe("premium_incomestart1317.31317.3");
  });

  it("highlights the bar the trace panel is pointing at", () => {
    render(<Waterfall flows={flows} t={3} highlight="model.death_claims" />);
    expect(bars()[1].getAttribute("class")!).toContain("is-highlight");
    expect(bars()[0].getAttribute("class")!).not.toContain("is-highlight");
  });
});
