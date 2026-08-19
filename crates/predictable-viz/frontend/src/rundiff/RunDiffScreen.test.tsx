import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ApiError, type DataSource } from "../datasource";
import { RunDiffScreen } from "./RunDiffScreen";
import { WALKTHROUGH_DIFF, WalkthroughSource } from "./__fixtures__";

const A = WALKTHROUGH_DIFF.a.run;
const B = WALKTHROUGH_DIFF.b.run;

function mount(source: DataSource = new WalkthroughSource(), props = {}) {
  return render(<RunDiffScreen source={source} a={A} b={B} {...props} />);
}

describe("Screen 2 — Run Diff, against the walkthrough's real diff.json", () => {
  it("reconciles the manifests before showing a number", async () => {
    mount();
    const banner = await screen.findByLabelText("manifest reconciliation");
    expect(within(banner).getByTestId("verdict")).toHaveTextContent("diverged");
    expect(banner.querySelector('[data-check="modelpoints"]')).toHaveAttribute(
      "data-status",
      "same",
    );
    expect(banner.querySelector('[data-check="model"]')).toHaveAttribute("data-status", "different");
    expect(within(banner).getByTestId("headline")).toHaveTextContent(
      "25 modelpoints · 13 outputs · 7 match within tolerance · 6 differ",
    );
    // Untouched outputs are stated as exact, not omitted.
    expect(banner.querySelector('[data-output="model.death_claims"]')).toHaveAttribute(
      "data-within",
      "true",
    );
  });

  it("selects the root finding first and attributes it to the model change", async () => {
    mount();
    const attribution = await screen.findByLabelText("attribution");
    expect(within(attribution).getByTestId("attribution-sentence")).toHaveTextContent(
      "an upstream change to premium_income explains this movement",
    );
    expect(within(attribution).getByTestId("attribution-impact")).toHaveTextContent(
      /impact set: \d+ components, reaching/,
    );
    expect(within(attribution).getByTestId("explain-command")).toHaveTextContent(
      "predictable explain",
    );
    // The H0101 hypothesis and its suggested edit come straight from the diff.
    expect(within(attribution).getByTestId("edit-H0101")).toHaveTextContent(
      "0.970873786407767",
    );
  });

  it("ranks the real modelpoints and labels the ranking as derived", async () => {
    mount();
    const panel = await screen.findByLabelText("contributors");
    await waitFor(() => expect(panel.querySelectorAll(".rd-bar-row").length).toBeGreaterThan(0));
    const worst = WALKTHROUGH_DIFF.findings[0].worst.mp_key;
    expect(panel.querySelectorAll(".rd-bar-row")[0]).toHaveAttribute("data-mp", worst);
    expect(within(panel).getByTestId("contributor-method")).toHaveTextContent(
      "derived in the browser",
    );
    // 25 of 25 modelpoints moved: there is no cohort, and the screen says so.
    await waitFor(() =>
      expect(within(panel).getByTestId("cohort")).toHaveAttribute("data-kind", "all"),
    );
    expect(within(panel).getByTestId("cohort")).toHaveTextContent("all 25 modelpoints differ");
  });

  it("switching to an inherited finding refuses to call it the cause", async () => {
    mount();
    await screen.findByLabelText("attribution");
    await userEvent.click(await screen.findByRole("button", { name: /F002/ }));
    await waitFor(() =>
      expect(screen.getByTestId("attribution-sentence")).toHaveTextContent(
        /reserve moved because .* it is not the cause/,
      ),
    );
  });

  it("opens the exemplar side by side with both explain() traces", async () => {
    const source = new WalkthroughSource();
    mount(source);
    await screen.findByLabelText("attribution");
    await userEvent.click(screen.getByRole("button", { name: /open TA00001 side-by-side/ }));

    const panel = await screen.findByLabelText("side by side TA00001");
    await waitFor(() =>
      expect(within(panel).getByTestId("sbs-summary")).toHaveTextContent(
        /first divergence at t=0/,
      ),
    );
    // The t=0 column carries the first-divergence marker.
    expect(panel.querySelector(".rd-first-divergence")).toHaveTextContent("t=0");
    // Both sides' traces are on screen, and each was asked of its own run.
    const traces = await within(panel).findAllByLabelText("explain trace");
    expect(traces).toHaveLength(2);
    expect(source.calls.explain.map((c) => c.run).sort()).toEqual([A, B].sort());
    expect(within(panel).getByTestId("trace-divergence")).toBeInTheDocument();
  });

  it("asks each side's series of that side's own run", async () => {
    const source = new WalkthroughSource();
    mount(source);
    await screen.findByLabelText("contributors");
    await waitFor(() => expect(source.calls.series.length).toBeGreaterThanOrEqual(2));
    const runs = new Set(source.calls.series.map((c) => c.run));
    expect(runs.has(A)).toBe(true);
    expect(runs.has(B)).toBe(true);
  });

  it("still renders the whole diff when no graph is available", async () => {
    const source = new WalkthroughSource(WALKTHROUGH_DIFF, null);
    mount(source);
    const attribution = await screen.findByLabelText("attribution");
    expect(within(attribution).queryByTestId("attribution-impact")).toBeNull();
    expect(within(attribution).getByTestId("attribution-sentence")).toBeInTheDocument();
  });

  it("says what to do when the source cannot diff at all", async () => {
    const source = new WalkthroughSource();
    vi.spyOn(source, "diff").mockRejectedValue(
      new ApiError(501, "unsupported", "not in this pack"),
    );
    mount(source);
    expect(await screen.findByText(/--include diff:/)).toBeInTheDocument();
  });

  it("hands `show in graph` back to the shell instead of navigating itself", async () => {
    const onShowInGraph = vi.fn();
    mount(new WalkthroughSource(), { onShowInGraph });
    await screen.findByLabelText("attribution");
    await userEvent.click(screen.getByRole("button", { name: "show in graph" }));
    expect(onShowInGraph).toHaveBeenCalledWith("model.premium_income");
  });
});
