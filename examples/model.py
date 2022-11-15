# `pip install -e .` to install the library in development
# import the library

import predictable as dv


def handler(modelpoint, **kwargs):
    # NOTE: **kwargs used to access info from RunConfig e.g.
    # NOTE: `table_location` can be used in TableLookup components
    # NOTE: `results_location` can be used to store results on a modelpoint level

    # Create new model instance
    model = dv.Model()

    # Add static components
    age = dv.RatingFactor(
        input_array=[modelpoint.age],
        formula=lambda prev: prev + 1,
        label="age",
    )
    gender = dv.RatingFactor(input_array=[modelpoint.gender], label="gender")
    smoker = dv.RatingFactor(
        input_array=[modelpoint.smoker_status], label="smoker"
    )

    # Add components that have a projected value
    prem = dv.CashFlow(
        input_array=[modelpoint.premium],
        formula=lambda prev: prev * 1.05,
        label="premium",
    )
    cover = dv.CashFlow(input_array=[modelpoint.cover], label="cover")
    exp = dv.StaticFlow(
        input_array=[modelpoint.expenses for _ in range(5)],
        label="expense",
    )

    # Add actuarial components
    qx = dv.TableLookup(
        table_name=kwargs["table_location"] / "sa8990.csv",
        lookup_on=["age"],
        keep_column="qx",
        label="qx",
    )
    lapses = dv.TableLookup(
        table_name=kwargs["table_location"] / "lapses.csv",
        lookup_on=["t"],
        keep_column="lapse_rate_pa",
        default_value=0,
        label="lapse",
    )

    v = dv.DiscountFactors(interest_rate=0.05, label="V")

    # Attach components to the model
    component_list = [age, gender, smoker, prem, cover, exp, v, qx, lapses]
    model.add_components(component_list)

    # Project over a given term
    # Results return a pandas df object
    df = model.project(term=33 - modelpoint.age)

    # Perform linear combination style manipulations
    components = ["premium", "cover", "expense"]
    for component in components:
        # NOTE: The logic here is just for illustration purposes
        df[f"EPV_{component}"] = (
            df[component] * df["V"] * df["qx"] * (1 - df["lapse"])
        )

    # Define reserving relationship
    df["EPV_BEL"] = df["EPV_cover"] + df["EPV_expense"] - df["EPV_premium"]

    df_out = df[["EPV_premium", "EPV_cover", "EPV_expense", "EPV_BEL"]].sum()

    print(df)

    # create an index
    df_out["policy_number"] = modelpoint.policy_number

    # TODO: Consider allowing a results dataclass to be specified
    # TODO: Still deciding return format i.e. dict or pandas object
    return df_out.to_dict()
