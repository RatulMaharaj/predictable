"""The library skeleton: a hook a product must fill in (`02-dsl.md` §7.2).

`@abstract` is DSL-only. There is no `Kind::Abstract` in the IR — an undischarged abstract fails
`predictable build` with `E1203` and no `.pir` is written (`01-ir.md` §13 Q10), so the resolve pass
never sees a hole.
"""

from predictable import abstract, series
from predictable.timing import END
from predictable.units import Money, Years


@abstract
@series(timing=END)
def surrender_value(policy_year: Years, sum_assured: Money) -> Money:
    """Product-specific: the library defines the hook, the product fills it in."""
