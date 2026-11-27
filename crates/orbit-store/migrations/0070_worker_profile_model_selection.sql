-- Worker profiles used to store a provider/model pair, but execution model resolution has
-- always been driven by the agent/team spec. The profile copy was unused, could drift from
-- the provider catalog, and made provider deletion look blocked. Keep the columns for
-- compatibility with older imports, but clear the stale values.
UPDATE worker_profiles SET provider = NULL, model = NULL;
