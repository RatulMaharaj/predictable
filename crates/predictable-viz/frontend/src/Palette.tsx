// The `Cmd-K` command palette — 05-viz.md §2.3.
//
// It searches component names, docs, tags *and Prophet variable names*, and a
// Prophet hit says so on the row: during a migration the actuary types
// `NUM_POLS_IF` and expects to land on whichever predictable component claims it.

import { useEffect, useMemo, useRef, useState } from "react";
import type { GraphDoc } from "./datasource";
import { buildSearchIndex, search, type SearchHit } from "./graph/search";

function Highlight({ text, positions }: { text: string; positions: number[] }) {
  const marked = new Set(positions);
  return (
    <>
      {Array.from(text).map((ch, i) =>
        marked.has(i) ? (
          <mark key={i}>{ch}</mark>
        ) : (
          <span key={i}>{ch}</span>
        ),
      )}
    </>
  );
}

function why(hit: SearchHit): string {
  switch (hit.field) {
    case "prophet":
      return `Prophet ${hit.text}`;
    case "tag":
      return `tag ${hit.text}`;
    case "module":
      return `module ${hit.text}`;
    case "doc":
      return hit.text;
    case "expr":
      return `expr ${hit.text}`;
    default:
      return hit.node.module;
  }
}

export function Palette({
  doc,
  open,
  onClose,
  onPick,
  initialQuery = "",
}: {
  doc: GraphDoc;
  open: boolean;
  onClose: () => void;
  onPick: (id: string, query: string) => void;
  initialQuery?: string;
}) {
  const [query, setQuery] = useState(initialQuery);
  const [cursor, setCursor] = useState(0);
  const input = useRef<HTMLInputElement | null>(null);
  const entries = useMemo(() => buildSearchIndex(doc), [doc]);
  const hits = useMemo(() => search(entries, query), [entries, query]);

  useEffect(() => {
    if (open) input.current?.focus();
  }, [open]);
  useEffect(() => setCursor(0), [query]);

  if (!open) return null;

  return (
    <div className="palette-backdrop" onClick={onClose} data-testid="palette">
      <div className="palette" onClick={(e) => e.stopPropagation()} role="dialog" aria-label="command palette">
        <input
          ref={input}
          value={query}
          placeholder="component, tag, doc, or Prophet variable…"
          aria-label="search components"
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setCursor((c) => Math.min(c + 1, hits.length - 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setCursor((c) => Math.max(c - 1, 0));
            } else if (e.key === "Enter") {
              e.preventDefault();
              if (hits[cursor]) onPick(hits[cursor].id, query);
            } else if (e.key === "Escape") {
              onClose();
            }
          }}
        />
        <ul role="listbox" aria-label="results">
          {hits.length === 0 && <li className="empty">no component matches</li>}
          {hits.map((hit, i) => (
            <li
              key={hit.id}
              role="option"
              aria-selected={i === cursor}
              className={i === cursor ? "active" : undefined}
              data-testid="palette-hit"
              data-id={hit.id}
              data-field={hit.field}
              onMouseEnter={() => setCursor(i)}
              onClick={() => onPick(hit.id, query)}
            >
              <span className="hit-name">
                {hit.field === "name" ? (
                  <Highlight text={hit.text} positions={hit.positions} />
                ) : (
                  hit.node.name
                )}
              </span>
              <span className={`hit-why hit-${hit.field}`}>{why(hit)}</span>
              <span className="hit-kind">{hit.node.kind}</span>
            </li>
          ))}
        </ul>
      </div>
    </div>
  );
}
