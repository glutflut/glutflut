// Region and start junction of the browser e2e scripts: Konstanz (Kaiserbrunnen)
// unless E2E_REGION / E2E_AT ("lat,lon") say otherwise.
export const REGION = process.env.E2E_REGION ?? "region-konstanz";
export const AT = process.env.E2E_AT ?? "47.6603493,9.1749263";
/** `path` with `region=` for the API. */
export const inRegion = (path) => `${path}${path.includes("?") ? "&" : "?"}region=${REGION}`;
