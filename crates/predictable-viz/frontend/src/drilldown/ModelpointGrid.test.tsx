import { describe, expect, it, vi } from "vitest";
import { useState } from "react";
import { fireEvent, render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { ModelpointGrid } from "./ModelpointGrid";
import type { Cell, Selection, SeriesMatrix } from "./grid";

const matrix: SeriesMatrix = {
  components: ["num_pols_if", "premium_income", "death_claims", "reserve"],
  periods: [0, 1, 2, 3],
  values: [
    [1, 0.9721, 0.9447, 0.9178],
    [1240, 1265.7, 1291.4, 1317.3],
    [null, 134.2, 138.9, 143.7],
    [1084.2, 1131.8, 1180.1, 1284.5512345],
  ],
};

function Harness(props: { onCopy?: (tsv: string) => void; onOpenCell?: (cell: Cell) => void; heat?: boolean }) {
  const [selection, setSelection] = useState<Selection>({
    anchor: { row: 0, col: 0 },
    focus: { row: 0, col: 0 },
  });
  return (
    <ModelpointGrid
      matrix={matrix}
      selection={selection}
      onSelectionChange={setSelection}
      {...props}
    />
  );
}

describe("ModelpointGrid", () => {
  it("renders components on rows and t on columns", () => {
    render(<Harness />);
    expect(screen.getByText("reserve")).toBeDefined();
    expect(screen.getByText("t=3")).toBeDefined();
    expect(screen.getByText("1,284.5512")).toBeDefined();
  });

  it("shows the exact f64 on hover, because 2 dp is not a reconciliation", () => {
    render(<Harness />);
    expect(screen.getByText("1,284.5512").getAttribute("title")).toBe("1284.5512345");
  });

  it("shows an em-dash, not a zero, where the engine emitted nothing", () => {
    render(<Harness />);
    const cells = screen.getAllByText("—");
    expect(cells).toHaveLength(1);
    expect(cells[0].getAttribute("title")).toBeNull();
  });

  it("mounts only the visible window", () => {
    const wide: SeriesMatrix = {
      components: Array.from({ length: 400 }, (_, i) => `c${i}`),
      periods: Array.from({ length: 400 }, (_, i) => i),
      values: Array.from({ length: 400 }, () => Array.from({ length: 400 }, () => 1)),
    };
    render(
      <ModelpointGrid
        matrix={wide}
        selection={{ anchor: { row: 0, col: 0 }, focus: { row: 0, col: 0 } }}
        onSelectionChange={() => {}}
      />,
    );
    expect(screen.getAllByRole("gridcell").length).toBeLessThan(400 * 400);
    expect(screen.queryByText("c399")).toBeNull();
    expect(screen.getByRole("grid").getAttribute("data-window")).toBe("0:24/0:14");
  });

  it("moves the selection with the arrow keys and marks the focused cell", async () => {
    const user = userEvent.setup();
    render(<Harness />);
    const grid = screen.getByRole("grid");
    grid.focus();
    await user.keyboard("{ArrowDown}{ArrowRight}");
    const focused = document.querySelector(".mp-grid-cell.is-focus")!;
    expect(focused.textContent).toBe("1,265.7");
    expect(focused.getAttribute("aria-selected")).toBe("true");
  });

  it("extends the selection with shift and copies it as TSV on ⌘C", async () => {
    const user = userEvent.setup();
    const onCopy = vi.fn();
    render(<Harness onCopy={onCopy} />);
    const grid = screen.getByRole("grid");
    grid.focus();
    await user.keyboard("{Shift>}{ArrowDown}{ArrowRight}{/Shift}");
    await user.keyboard("{Meta>}c{/Meta}");
    expect(onCopy).toHaveBeenCalledWith("component\tt=0\tt=1\nnum_pols_if\t1\t0.9721\npremium_income\t1240\t1265.7");
  });

  it("copies from the button too, for the mouse path", async () => {
    const user = userEvent.setup();
    const onCopy = vi.fn();
    render(<Harness onCopy={onCopy} />);
    await user.click(screen.getByRole("button", { name: "copy TSV" }));
    expect(onCopy).toHaveBeenCalledWith("component\tt=0\nnum_pols_if\t1");
  });

  it("opens explain() for the focused cell on Enter", async () => {
    const user = userEvent.setup();
    const onOpenCell = vi.fn();
    render(<Harness onOpenCell={onOpenCell} />);
    const grid = screen.getByRole("grid");
    grid.focus();
    await user.keyboard("{ArrowDown}{Enter}");
    expect(onOpenCell).toHaveBeenCalledWith({ row: 1, col: 0 });
  });

  it("selects a rectangle with shift-click", () => {
    render(<Harness />);
    const cells = screen.getAllByRole("gridcell");
    fireEvent.mouseDown(cells[0]);
    fireEvent.mouseDown(screen.getByText("138.9"), { shiftKey: true });
    expect(document.querySelectorAll('[aria-selected="true"]')).toHaveLength(9);
  });

  it("washes cells with heat only when the overlay is on", () => {
    const { rerender } = render(<Harness />);
    expect(document.querySelector("[data-heat]")).toBeNull();
    rerender(<Harness heat />);
    const hottest = screen.getByText("1,284.5512");
    expect(hottest.getAttribute("data-heat")).toBe("1.000");
    // an absent value gets no heat rather than false heat
    expect(screen.getByText("—").getAttribute("data-heat")).toBeNull();
  });
});
