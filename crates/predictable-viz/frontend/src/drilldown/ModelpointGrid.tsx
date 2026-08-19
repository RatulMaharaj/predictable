import { useCallback, useLayoutEffect, useRef, useState } from "react";
import {
  cellText,
  DEFAULT_METRICS,
  heatIntensity,
  isSelected,
  moveSelection,
  selectionToTsv,
  visibleWindow,
  type Cell,
  type Metrics,
  type Selection,
  type SeriesMatrix,
  type Viewport,
} from "./grid";
import { formatExact } from "./trace";

export interface ModelpointGridProps {
  matrix: SeriesMatrix;
  selection: Selection;
  onSelectionChange: (selection: Selection) => void;
  /** Selecting a cell is the entry point to `explain()` (§3.4 level 3). */
  onOpenCell?: (cell: Cell) => void;
  heat?: boolean;
  metrics?: Metrics;
  /** Injected so the copy path is testable without a clipboard permission. */
  onCopy?: (tsv: string) => void;
}

/**
 * Level 3 of the drill-down: components on rows, `t` on columns, virtualised,
 * keyboard-navigable, with a heat overlay toggle and `Cmd-C` TSV copy.
 *
 * "Actuaries will do this constantly and it must be perfect" (§4.3), so the
 * copy carries the engine's full `f64` and an empty cell copies as empty.
 */
export function ModelpointGrid({
  matrix,
  selection,
  onSelectionChange,
  onOpenCell,
  heat = false,
  metrics = DEFAULT_METRICS,
  onCopy,
}: ModelpointGridProps) {
  const scroller = useRef<HTMLDivElement>(null);
  const [viewport, setViewport] = useState<Viewport>({
    scrollTop: 0,
    scrollLeft: 0,
    height: 480,
    width: 960,
  });

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    // jsdom reports zero here; the fallback keeps the window non-empty so the
    // grid is testable and a zero-height first paint still renders rows.
    setViewport((v) => ({
      ...v,
      height: el.clientHeight || v.height,
      width: el.clientWidth || v.width,
    }));
  }, []);

  const onScroll = useCallback(() => {
    const el = scroller.current;
    if (!el) return;
    setViewport((v) => ({ ...v, scrollTop: el.scrollTop, scrollLeft: el.scrollLeft }));
  }, []);

  const copy = useCallback(() => {
    const tsv = selectionToTsv(matrix, selection);
    if (onCopy) onCopy(tsv);
    else void navigator.clipboard?.writeText(tsv);
  }, [matrix, selection, onCopy]);

  const onKeyDown = useCallback(
    (event: React.KeyboardEvent<HTMLDivElement>) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "c") {
        event.preventDefault();
        copy();
        return;
      }
      if (event.key === "Enter") {
        event.preventDefault();
        onOpenCell?.(selection.focus);
        return;
      }
      const next = moveSelection(matrix, selection, event.key, event.shiftKey);
      if (!next) return;
      event.preventDefault();
      onSelectionChange(next);
    },
    [copy, matrix, onOpenCell, onSelectionChange, selection],
  );

  const window_ = visibleWindow(matrix, viewport, metrics);
  const { rowHeight, colWidth } = metrics;
  const rows: number[] = [];
  for (let r = window_.rowStart; r < window_.rowEnd; r++) rows.push(r);
  const cols: number[] = [];
  for (let c = window_.colStart; c < window_.colEnd; c++) cols.push(c);

  return (
    <div className="mp-grid">
      <div
        className="mp-grid-scroll"
        ref={scroller}
        onScroll={onScroll}
        onKeyDown={onKeyDown}
        tabIndex={0}
        role="grid"
        aria-label="modelpoint grid"
        aria-rowcount={matrix.components.length}
        aria-colcount={matrix.periods.length}
        data-window={`${window_.rowStart}:${window_.rowEnd}/${window_.colStart}:${window_.colEnd}`}
      >
        <div
          className="mp-grid-canvas"
          style={{
            height: matrix.components.length * rowHeight,
            width: matrix.periods.length * colWidth,
          }}
        >
          <div className="mp-grid-head" style={{ height: rowHeight }}>
            {cols.map((c) => (
              <div
                key={c}
                className="mp-grid-th"
                style={{ left: c * colWidth, width: colWidth, height: rowHeight }}
              >
                t={matrix.periods[c]}
              </div>
            ))}
          </div>
          {rows.map((r) => (
            <div
              key={r}
              className="mp-grid-row"
              role="row"
              style={{ top: r * rowHeight, height: rowHeight }}
            >
              <div className="mp-grid-label" title={matrix.components[r]}>
                {matrix.components[r]}
              </div>
              {cols.map((c) => {
                const value = matrix.values[r][c];
                const selected = isSelected(selection, r, c);
                const focused = selection.focus.row === r && selection.focus.col === c;
                const intensity = heat ? heatIntensity(matrix, r, c) : null;
                return (
                  <div
                    key={c}
                    role="gridcell"
                    aria-selected={selected}
                    className={`mp-grid-cell${selected ? " is-selected" : ""}${
                      focused ? " is-focus" : ""
                    }${value === null ? " is-empty" : ""}`}
                    style={{
                      left: c * colWidth,
                      width: colWidth,
                      height: rowHeight,
                      // The overlay is an alpha wash under the text, so the
                      // number never loses contrast (§5.2).
                      ...(intensity === null
                        ? {}
                        : { backgroundColor: `rgba(56, 132, 255, ${(intensity * 0.45).toFixed(3)})` }),
                    }}
                    data-heat={intensity === null ? undefined : intensity.toFixed(3)}
                    title={value === null ? undefined : formatExact(value)}
                    onMouseDown={(e) => {
                      const cell = { row: r, col: c };
                      onSelectionChange(
                        e.shiftKey ? { anchor: selection.anchor, focus: cell } : { anchor: cell, focus: cell },
                      );
                    }}
                    onDoubleClick={() => onOpenCell?.({ row: r, col: c })}
                  >
                    {cellText(value)}
                  </div>
                );
              })}
            </div>
          ))}
        </div>
      </div>
      <div className="mp-grid-actions">
        <button type="button" onClick={copy}>
          copy TSV
        </button>
        <span className="hint">⌘C copies the selection · ↵ opens explain()</span>
      </div>
    </div>
  );
}
