! ---------------------------------------------------------------------------
! TERM_UK.MOD  —  library source extract, Prophet 2023.2
! Product: level term assurance, annual projection, 40 periods
! Run settings: TERM_BASE_2026Q2, valuation date 30/06/2026
!
! This is the file the migration reads in step 1 of skills/predictable-migration.
! It is the authority for every formula; the .rpt is the authority for every number.
! ---------------------------------------------------------------------------

! --- run parameters -------------------------------------------------------
VAL_RATE      = 0.035        ! flat valuation rate p.a.
MORT_LOAD     = 1.05         ! loading on the table rate
LAPSE_LOAD    = 1.00
EXP_INFL      = 0.028        ! expense inflation p.a.
EXP_PP        = 48.00        ! renewal expense per policy p.a. at the valuation date
INIT_EXP_PCT  = 1.35         ! acquisition expense as a multiple of the first premium

! --- decrements -----------------------------------------------------------
ATT_AGE       = AGE_AT_ENTRY + t
IN_TERM       = IF t < POL_TERM THEN 1 ELSE 0
MORT_RATE     = SA8990(ATT_AGE, SEX, SMOKER) * MORT_LOAD
LAPSE_RATE    = LAPSE_TERM(t + 1) * LAPSE_LOAD

! Death first, then lapse on the survivors. The two decrements do not compete
! for the same exposure.
POLS_IF       = IF t = 0 THEN 1
                ELSE POLS_IF(t-1) * (1 - MORT_RATE(t-1)) * (1 - LAPSE_RATE(t-1)) * IN_TERM

DEATHS        = POLS_IF * MORT_RATE * IN_TERM
SURRENDERS    = POLS_IF * (1 - MORT_RATE) * LAPSE_RATE * IN_TERM

! --- cashflows ------------------------------------------------------------
! PREM_INC is received in advance at the start of the policy year.
PREM_INC      = ANN_PREM * POLS_IF * IN_TERM
! DTH_CLAIM is paid at the end of the policy year.
DTH_CLAIM     = SUM_ASSURED * DEATHS
! EXPENSE is incurred in advance and inflated from the valuation date, so the
! exponent at t = 0 is zero: the first year's expense is NOT inflated.
EXPENSE       = EXP_PP * (1 + EXP_INFL)^t * EXP_BAND(EXP_BAND_CD) * POLS_IF * IN_TERM
NET_CF        = PREM_INC - DTH_CLAIM - EXPENSE

! --- valuation ------------------------------------------------------------
DISC          = IF t = 0 THEN 1 ELSE DISC(t-1) / (1 + VAL_RATE)
! Premiums and expenses are start-of-year flows and discount at DISC(t).
! Claims are end-of-year flows and discount one period further, at DISC(t+1).
PV_PREM       = SUMALL(PREM_INC * DISC)
PV_CLM        = SUMALL(DTH_CLAIM * DISC(t+1))
PV_EXP        = SUMALL(EXPENSE * DISC)
INIT_EXP      = INIT_EXP_PCT * ANN_PREM
BEL           = PV_CLM + PV_EXP + INIT_EXP - PV_PREM

! Retrospective roll-forward, seeded from the prospective BEL at t = 0.
RESERVE       = IF t = 0 THEN BEL
                ELSE (RESERVE(t-1) + PREM_INC(t-1) - DTH_CLAIM(t-1) - EXPENSE(t-1))
                     * (1 + VAL_RATE) * (IF t <= POL_TERM THEN 1 ELSE 0)

! Working variable, reported but not used downstream.
RESERVE_INT   = RESERVE * VAL_RATE
! ---------------------------------------------------------------------------
