-- Glut & Flut schema, first version (2026-10-15): everything the earlier
-- migrations built, in one file. Run by db/flyway.sh (bootstrap on an empty
-- database). Needs PostGIS (with raster and topology), H3 (with h3_postgis)
-- and pgRouting.
--
-- Schemas: geo (regions, street graph, places, finds, media), game (players,
-- scores, campaigns, keeps), analytics (k-anonymous aggregates, nothing per
-- person), ops (dashboard tokens, audit log, cells), terrain and util (import
-- helpers), core (factions; an early table the graph still refers to).
--
-- Roles, made by hand before this runs (missing ones are skipped):
--   elements_owner  Flyway and imports (owns everything, DDL)
--   elements_app    the game server: reads geo, reads and writes game and
--                   analytics, a few geo and ops tables; no DDL
--   elements_read   analysis: geo and analytics, nothing about players
-- Default privileges cover tables later migrations add (run as owner).

SET check_function_bodies = false;

CREATE SCHEMA analytics;

CREATE SCHEMA core;

CREATE SCHEMA game;

CREATE SCHEMA geo;

CREATE SCHEMA ops;

CREATE SCHEMA terrain;

CREATE SCHEMA topology;

CREATE SCHEMA util;

CREATE EXTENSION IF NOT EXISTS h3 WITH SCHEMA public;

CREATE EXTENSION IF NOT EXISTS postgis WITH SCHEMA public;

CREATE EXTENSION IF NOT EXISTS postgis_raster WITH SCHEMA public;

CREATE EXTENSION IF NOT EXISTS h3_postgis WITH SCHEMA public;

CREATE EXTENSION IF NOT EXISTS pgrouting WITH SCHEMA public;

CREATE EXTENSION IF NOT EXISTS postgis_topology WITH SCHEMA topology;

CREATE FUNCTION util.uid() RETURNS text
    LANGUAGE sql STABLE
    AS $$
  SELECT nullif(current_setting('request.jwt.claims', true), '')::json->>'sub'
$$;

CREATE TABLE analytics.area_hour (
    region text NOT NULL,
    hour_start timestamp with time zone NOT NULL,
    minutes smallint NOT NULL,
    fire_ha real NOT NULL,
    water_ha real NOT NULL,
    fire_player_ha real NOT NULL,
    water_player_ha real NOT NULL,
    fire_max_ha real NOT NULL,
    water_max_ha real NOT NULL
);

CREATE TABLE analytics.block_day (
    region text NOT NULL,
    day date NOT NULL,
    block_id integer NOT NULL,
    fire_min integer DEFAULT 0 NOT NULL,
    water_min integer DEFAULT 0 NOT NULL
);

CREATE TABLE analytics.daily (
    region text NOT NULL,
    day date NOT NULL,
    synthetic boolean DEFAULT false NOT NULL,
    players integer NOT NULL,
    newcomers integer DEFAULT 0 NOT NULL,
    flagged integer DEFAULT 0 NOT NULL,
    CONSTRAINT daily_players_check CHECK ((players >= 0))
);

CREATE TABLE analytics.daily_profile (
    region text NOT NULL,
    day date NOT NULL,
    synthetic boolean DEFAULT false NOT NULL,
    age text NOT NULL,
    local text NOT NULL,
    players integer NOT NULL,
    CONSTRAINT daily_profile_age_check CHECK ((age = ANY (ARRAY['u30'::text, '30_59'::text, '60p'::text, 'na'::text]))),
    CONSTRAINT daily_profile_local_check CHECK ((local = ANY (ARRAY['yes'::text, 'no'::text, 'na'::text]))),
    CONSTRAINT daily_profile_players_check CHECK ((players >= 0))
);

CREATE TABLE analytics.dwell (
    region text NOT NULL,
    edge_id bigint NOT NULL,
    window_start timestamp with time zone NOT NULL,
    synthetic boolean DEFAULT false NOT NULL,
    persons integer NOT NULL,
    dwell_s integer NOT NULL,
    hotspot_dwell_s integer DEFAULT 0 NOT NULL,
    span_s integer DEFAULT 900 NOT NULL,
    CONSTRAINT dwell_dwell_s_check CHECK ((dwell_s >= 0)),
    CONSTRAINT dwell_hotspot_dwell_s_check CHECK ((hotspot_dwell_s >= 0)),
    CONSTRAINT dwell_persons_check CHECK ((persons > 0)),
    CONSTRAINT dwell_span_s_check CHECK ((span_s > 0))
);

CREATE TABLE analytics.flow (
    region text NOT NULL,
    from_zone text NOT NULL,
    to_zone text NOT NULL,
    hour_start timestamp with time zone NOT NULL,
    synthetic boolean DEFAULT false NOT NULL,
    persons integer NOT NULL,
    CONSTRAINT flow_persons_check CHECK ((persons > 0))
);

CREATE TABLE analytics.flow_day (
    region text NOT NULL,
    from_zone text NOT NULL,
    to_zone text NOT NULL,
    day date NOT NULL,
    synthetic boolean NOT NULL,
    persons integer NOT NULL
);

CREATE TABLE analytics.growth_day (
    region text NOT NULL,
    day date NOT NULL,
    players integer NOT NULL,
    new integer NOT NULL,
    multi integer NOT NULL,
    total integer NOT NULL
);

CREATE TABLE analytics.hex_dwell (
    region text NOT NULL,
    res smallint NOT NULL,
    cell bigint NOT NULL,
    hour_start timestamp with time zone NOT NULL,
    synthetic boolean DEFAULT false NOT NULL,
    persons integer NOT NULL,
    dwell_s integer NOT NULL,
    hotspot_dwell_s integer DEFAULT 0 NOT NULL,
    span_s integer DEFAULT 3600 NOT NULL,
    CONSTRAINT hex_dwell_dwell_s_check CHECK ((dwell_s >= 0)),
    CONSTRAINT hex_dwell_hotspot_dwell_s_check CHECK ((hotspot_dwell_s >= 0)),
    CONSTRAINT hex_dwell_persons_check CHECK ((persons > 0)),
    CONSTRAINT hex_dwell_span_s_check CHECK ((span_s > 0))
);

CREATE TABLE analytics.hex_suppressed (
    region text NOT NULL,
    res smallint NOT NULL,
    hour_start timestamp with time zone NOT NULL,
    synthetic boolean DEFAULT false NOT NULL,
    kept integer NOT NULL,
    dropped integer NOT NULL
);

CREATE TABLE analytics.interventions (
    id bigint NOT NULL,
    region text NOT NULL,
    node_id bigint NOT NULL,
    starts_at timestamp with time zone NOT NULL,
    ends_at timestamp with time zone NOT NULL,
    source text NOT NULL,
    CONSTRAINT interventions_source_check CHECK ((source = ANY (ARRAY['random'::text, 'manual'::text, 'planned'::text])))
);

CREATE SEQUENCE analytics.interventions_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE analytics.interventions_id_seq OWNED BY analytics.interventions.id;

CREATE TABLE analytics.link_day (
    day date NOT NULL,
    code text NOT NULL,
    hits integer DEFAULT 0 NOT NULL
);

CREATE TABLE analytics.perception (
    region text NOT NULL,
    node_id bigint NOT NULL,
    day date NOT NULL,
    category text NOT NULL,
    synthetic boolean DEFAULT false NOT NULL,
    count integer NOT NULL,
    hour smallint NOT NULL,
    CONSTRAINT perception_count_check CHECK ((count > 0)),
    CONSTRAINT perception_hour_check CHECK (((hour >= '-1'::integer) AND (hour <= 23)))
);

CREATE TABLE analytics.place_visits (
    region text NOT NULL,
    zone_id text NOT NULL,
    start timestamp with time zone NOT NULL,
    span_s integer NOT NULL,
    synthetic boolean NOT NULL,
    persons integer NOT NULL,
    dwell_s integer NOT NULL,
    CONSTRAINT place_visits_span_s_check CHECK ((span_s = ANY (ARRAY[3600, 86400])))
);

CREATE TABLE analytics.site_visits (
    region text NOT NULL,
    site_node bigint NOT NULL,
    slot_start timestamp with time zone NOT NULL,
    synthetic boolean DEFAULT false NOT NULL,
    active boolean NOT NULL,
    nearby integer NOT NULL,
    reached integer NOT NULL,
    reached_nearby integer DEFAULT 0 NOT NULL,
    CONSTRAINT site_visits_nearby_check CHECK ((nearby >= 0)),
    CONSTRAINT site_visits_reached_check CHECK ((reached >= 0)),
    CONSTRAINT site_visits_reached_nearby_check CHECK ((reached_nearby >= 0))
);

CREATE TABLE analytics.suppressed (
    region text NOT NULL,
    window_start timestamp with time zone NOT NULL,
    synthetic boolean DEFAULT false NOT NULL,
    kept integer NOT NULL,
    dropped integer NOT NULL,
    persons integer,
    CONSTRAINT suppressed_dropped_check CHECK ((dropped >= 0)),
    CONSTRAINT suppressed_kept_check CHECK ((kept >= 0))
);

CREATE TABLE core.factions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    slug text NOT NULL,
    name text NOT NULL,
    color text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE game.campaign_mottos (
    region text NOT NULL,
    num smallint NOT NULL,
    campaign text NOT NULL,
    side text NOT NULL,
    names jsonb NOT NULL,
    status text DEFAULT 'proposed'::text NOT NULL,
    created_by text,
    approved_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT campaign_mottos_num_check CHECK (((num >= 1) AND (num <= 999))),
    CONSTRAINT campaign_mottos_side_check CHECK ((side = ANY (ARRAY['fire'::text, 'water'::text, 'both'::text]))),
    CONSTRAINT campaign_mottos_status_check CHECK ((status = ANY (ARRAY['proposed'::text, 'approved'::text])))
);

CREATE TABLE game.chapter_progress (
    region text NOT NULL,
    key text NOT NULL,
    faction text NOT NULL,
    value bigint DEFAULT 0 NOT NULL,
    reached_at timestamp with time zone,
    CONSTRAINT chapter_progress_faction_check CHECK ((faction = ANY (ARRAY['fire'::text, 'water'::text])))
);

CREATE TABLE game.chapters (
    region text NOT NULL,
    key text NOT NULL,
    campaign text NOT NULL,
    names jsonb NOT NULL,
    starts timestamp with time zone NOT NULL,
    ends timestamp with time zone NOT NULL,
    goal text NOT NULL,
    target integer NOT NULL,
    target_water integer,
    kind text,
    status text DEFAULT 'published'::text NOT NULL,
    created_by text,
    nodes bigint[],
    CONSTRAINT chapters_check CHECK ((ends > starts)),
    CONSTRAINT chapters_goal_check CHECK ((goal = ANY (ARRAY['claims'::text, 'fronts'::text, 'finds'::text, 'points'::text, 'area'::text, 'hold'::text]))),
    CONSTRAINT chapters_key_check CHECK ((key ~ '^[a-z0-9-]+$'::text)),
    CONSTRAINT chapters_kind_check CHECK ((kind ~ '^[a-z]+:[a-z]+$'::text)),
    CONSTRAINT chapters_status_check CHECK ((status = ANY (ARRAY['draft'::text, 'published'::text]))),
    CONSTRAINT chapters_target_check CHECK ((target > 0)),
    CONSTRAINT chapters_target_water_check CHECK ((target_water > 0))
);

CREATE TABLE game.faction_day (
    region text NOT NULL,
    day date NOT NULL,
    faction text NOT NULL,
    node_minutes bigint DEFAULT 0 NOT NULL,
    points bigint DEFAULT 0 NOT NULL,
    claims bigint DEFAULT 0 NOT NULL,
    players integer DEFAULT 0 NOT NULL,
    CONSTRAINT faction_day_faction_check CHECK ((faction = ANY (ARRAY['fire'::text, 'water'::text])))
);

CREATE TABLE game.feedback (
    id bigint NOT NULL,
    region text NOT NULL,
    day date DEFAULT CURRENT_DATE NOT NULL,
    mood smallint NOT NULL,
    text text,
    lang text,
    version text,
    CONSTRAINT feedback_lang_check CHECK ((lang ~ '^[a-z]{2}$'::text)),
    CONSTRAINT feedback_mood_check CHECK (((mood >= 1) AND (mood <= 3))),
    CONSTRAINT feedback_text_check CHECK ((length(text) <= 1000)),
    CONSTRAINT feedback_version_check CHECK ((length(version) <= 60))
);

CREATE SEQUENCE game.feedback_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE game.feedback_id_seq OWNED BY game.feedback.id;

CREATE TABLE game.hotspot_plans (
    id bigint NOT NULL,
    region text NOT NULL,
    name text NOT NULL,
    node_id bigint NOT NULL,
    lat double precision NOT NULL,
    lon double precision NOT NULL,
    starts timestamp with time zone NOT NULL,
    ends timestamp with time zone NOT NULL,
    created_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT hotspot_plans_check CHECK (((ends > starts) AND ((ends - starts) <= '12:00:00'::interval))),
    CONSTRAINT hotspot_plans_name_check CHECK (((length(name) >= 1) AND (length(name) <= 60)))
);

CREATE SEQUENCE game.hotspot_plans_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE game.hotspot_plans_id_seq OWNED BY game.hotspot_plans.id;

CREATE TABLE game.keep_pot (
    region text NOT NULL,
    ticket bytea NOT NULL,
    points integer NOT NULL,
    week date NOT NULL
);

CREATE TABLE game.keep_resets (
    region text NOT NULL,
    at timestamp with time zone NOT NULL
);

CREATE TABLE game.keep_slots (
    region text NOT NULL,
    id bytea NOT NULL,
    slot bytea NOT NULL
);

CREATE TABLE game.keeps (
    region text NOT NULL,
    node_id bigint NOT NULL,
    faction text NOT NULL,
    age_s real NOT NULL,
    built_s real NOT NULL,
    ramp_s real DEFAULT 0 NOT NULL,
    wear real DEFAULT 0 NOT NULL,
    mult real DEFAULT 1 NOT NULL,
    points real DEFAULT 0 NOT NULL,
    ticket bytea NOT NULL,
    motto text,
    CONSTRAINT keeps_faction_check CHECK ((faction = ANY (ARRAY['fire'::text, 'water'::text]))),
    CONSTRAINT keeps_motto_check CHECK ((motto ~ '^[a-z]-[a-z0-9-]+$'::text))
);

CREATE TABLE game.player_badges (
    id bytea NOT NULL,
    badge text NOT NULL,
    CONSTRAINT player_badges_badge_check CHECK ((badge ~ '^[a-z0-9:-]+$'::text))
);

CREATE TABLE game.player_finds (
    id bytea NOT NULL,
    find bigint NOT NULL
);

CREATE TABLE game.player_presence (
    id bytea NOT NULL,
    code text NOT NULL
);

CREATE TABLE game.player_region (
    region text NOT NULL,
    id bytea NOT NULL,
    points_total bigint DEFAULT 0 NOT NULL,
    points_day bigint DEFAULT 0 NOT NULL,
    day date NOT NULL,
    last_seen date NOT NULL,
    week date,
    points_week bigint DEFAULT 0 NOT NULL,
    days smallint DEFAULT 1 NOT NULL,
    since date
);

CREATE TABLE game.players (
    region text NOT NULL,
    id bytea NOT NULL,
    faction text NOT NULL,
    name text NOT NULL,
    day date NOT NULL,
    hidden boolean DEFAULT false NOT NULL,
    last_seen date NOT NULL,
    recovery_hash bytea,
    xp_prev real DEFAULT 0 NOT NULL,
    xp_day_base real DEFAULT 0 NOT NULL,
    xp_day_claims real DEFAULT 0 NOT NULL,
    xp_day_kinds smallint DEFAULT 0 NOT NULL,
    collect boolean DEFAULT false NOT NULL,
    claims integer DEFAULT 0 NOT NULL,
    home text,
    home_id bytea,
    xp_since timestamp with time zone DEFAULT now() NOT NULL,
    streak_weeks smallint DEFAULT 0 NOT NULL,
    streak_week date,
    streak_days smallint DEFAULT 0 NOT NULL,
    streak_day date,
    streak_acts smallint DEFAULT 0 NOT NULL,
    streak_spared smallint DEFAULT 0 NOT NULL,
    CONSTRAINT players_faction_check CHECK ((faction = ANY (ARRAY['fire'::text, 'water'::text]))),
    CONSTRAINT players_id_check CHECK ((length(id) = 32)),
    CONSTRAINT players_recovery_hash_check CHECK ((length(recovery_hash) = 32))
);

CREATE TABLE game.presence_codes (
    code text NOT NULL,
    region text NOT NULL,
    lat double precision NOT NULL,
    lon double precision NOT NULL,
    radius_m real DEFAULT 150 NOT NULL,
    label text NOT NULL,
    active boolean DEFAULT true NOT NULL,
    created date DEFAULT CURRENT_DATE NOT NULL
);

CREATE TABLE game.shard_outbox (
    id bigint NOT NULL,
    shard text NOT NULL,
    body jsonb NOT NULL,
    tries integer DEFAULT 0 NOT NULL,
    next_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE SEQUENCE game.shard_outbox_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE game.shard_outbox_id_seq OWNED BY game.shard_outbox.id;

CREATE TABLE game.weather_days (
    region text NOT NULL,
    day date NOT NULL,
    tmax real NOT NULL,
    tmin real NOT NULL,
    precip_mm real NOT NULL,
    sun_h real NOT NULL,
    usual_tmax real NOT NULL
);

CREATE TABLE game.wishes (
    cell text NOT NULL,
    day date NOT NULL,
    count integer DEFAULT 0 NOT NULL
);

CREATE TABLE geo.blocks (
    region text NOT NULL,
    id integer NOT NULL,
    geom public.geometry(MultiPolygon,4326) NOT NULL,
    area_m2 real NOT NULL,
    nodes bigint[] NOT NULL
);

CREATE TABLE geo.commons_suggestions (
    region text NOT NULL,
    find bigint NOT NULL,
    title text NOT NULL,
    thumb_url text NOT NULL,
    page_url text NOT NULL,
    author text NOT NULL,
    licence text NOT NULL,
    licence_url text,
    distance_m real NOT NULL,
    fetched_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE geo.edges (
    id bigint NOT NULL,
    region_id uuid NOT NULL,
    source bigint NOT NULL,
    target bigint NOT NULL,
    geom public.geometry(LineString,4326) NOT NULL,
    length_m double precision DEFAULT 0 NOT NULL,
    road_type text,
    one_way boolean DEFAULT false NOT NULL,
    elev_from_m double precision,
    elev_to_m double precision,
    grade_forward double precision,
    grade_reverse double precision,
    water_cost_forward double precision,
    water_cost_reverse double precision,
    fire_cost_forward double precision,
    fire_cost_reverse double precision,
    base_travel_cost double precision,
    is_active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    bridge boolean DEFAULT false NOT NULL
);

CREATE TABLE geo.finds (
    id bigint NOT NULL,
    region text NOT NULL,
    collection text NOT NULL,
    name text NOT NULL,
    osm text NOT NULL,
    lat double precision NOT NULL,
    lon double precision NOT NULL,
    hidden boolean DEFAULT false NOT NULL,
    created_by text,
    updated_at timestamp with time zone,
    CONSTRAINT finds_collection_check CHECK ((collection ~ '^[a-z]+$'::text))
);

CREATE SEQUENCE geo.finds_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE geo.finds_id_seq OWNED BY geo.finds.id;

CREATE TABLE geo.hotspot_sites (
    id bigint NOT NULL,
    region text NOT NULL,
    name text NOT NULL,
    node_id bigint NOT NULL,
    lat double precision NOT NULL,
    lon double precision NOT NULL,
    created_by text,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT hotspot_sites_name_check CHECK (((length(name) >= 1) AND (length(name) <= 60)))
);

CREATE SEQUENCE geo.hotspot_sites_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE geo.hotspot_sites_id_seq OWNED BY geo.hotspot_sites.id;

CREATE TABLE geo.media (
    id bigint NOT NULL,
    region text NOT NULL,
    kind text NOT NULL,
    target text NOT NULL,
    s3_key text NOT NULL,
    width integer NOT NULL,
    height integer NOT NULL,
    bytes integer NOT NULL,
    author text NOT NULL,
    licence text NOT NULL,
    licence_version text,
    licence_url text,
    source_url text,
    commons_title text,
    alt text,
    status text DEFAULT 'pending'::text NOT NULL,
    uploaded_by text NOT NULL,
    approved_by text,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    decided_at timestamp with time zone,
    CONSTRAINT media_kind_check CHECK ((kind = ANY (ARRAY['find'::text, 'zone'::text, 'challenge'::text]))),
    CONSTRAINT media_licence_check CHECK ((licence = ANY (ARRAY['own'::text, 'cc-by'::text, 'cc-by-sa'::text, 'cc0'::text, 'pd'::text]))),
    CONSTRAINT media_second_person CHECK (((approved_by IS NULL) OR (approved_by <> uploaded_by) OR (approved_by = 'ops'::text))),
    CONSTRAINT media_status_check CHECK ((status = ANY (ARRAY['pending'::text, 'approved'::text, 'rejected'::text])))
);

CREATE SEQUENCE geo.media_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE geo.media_id_seq OWNED BY geo.media.id;

CREATE TABLE geo.node_tide (
    node_id bigint NOT NULL,
    dist_m real NOT NULL
);

CREATE TABLE geo.nodes (
    id bigint NOT NULL,
    region_id uuid NOT NULL,
    geom public.geometry(Point,4326) NOT NULL,
    lat double precision NOT NULL,
    lon double precision NOT NULL,
    elevation_m double precision DEFAULT 0 NOT NULL,
    node_type text DEFAULT 'crossing'::text NOT NULL,
    owner_faction_id uuid,
    control_state text DEFAULT 'neutral'::text NOT NULL,
    story_tag text,
    is_active boolean DEFAULT true NOT NULL,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    affinity real DEFAULT 0 NOT NULL,
    CONSTRAINT nodes_affinity_check CHECK (((affinity >= ('-1'::integer)::double precision) AND (affinity <= (1)::double precision)))
);

CREATE TABLE geo.region_tide (
    region text NOT NULL,
    source text DEFAULT 'pegelonline'::text NOT NULL,
    station text NOT NULL,
    mlw real NOT NULL,
    mhw real NOT NULL,
    CONSTRAINT region_tide_check CHECK ((mhw > mlw))
);

CREATE TABLE geo.regions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    slug text NOT NULL,
    name text NOT NULL,
    boundary_geom public.geometry(MultiPolygon,4326),
    centroid_geom public.geometry(Point,4326),
    bounds_min_lat double precision,
    bounds_min_lon double precision,
    bounds_max_lat double precision,
    bounds_max_lon double precision,
    active boolean DEFAULT false NOT NULL,
    terrain_version_id uuid,
    road_graph_version_id uuid,
    ruleset_version_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    hotspot_lat double precision,
    hotspot_lon double precision,
    hotspot_area_m real,
    demo_label text,
    demo_day date,
    timezone text DEFAULT 'Europe/Berlin'::text NOT NULL,
    language text DEFAULT 'de'::text NOT NULL,
    rules text,
    shard text DEFAULT 'eu'::text NOT NULL,
    listed boolean DEFAULT true NOT NULL,
    city_in jsonb,
    cell text DEFAULT 'main'::text NOT NULL,
    CONSTRAINT regions_hotspot_area_m_check CHECK ((hotspot_area_m > (0)::double precision)),
    CONSTRAINT regions_language_check CHECK ((language ~ '^[a-z]{2}$'::text)),
    CONSTRAINT regions_rules_check CHECK ((rules ~ '^[a-z0-9-]+$'::text)),
    CONSTRAINT regions_shard_check CHECK ((shard ~ '^[a-z0-9-]+$'::text))
);

CREATE TABLE geo.territories (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    region_id uuid NOT NULL,
    polygon_geom public.geometry(MultiPolygon,4326),
    owner_faction_id uuid,
    control_score double precision DEFAULT 0 NOT NULL,
    stability_score double precision DEFAULT 0 NOT NULL,
    contested_until timestamp with time zone,
    derived_from_version uuid,
    updated_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE geo.zones (
    region text NOT NULL,
    id text NOT NULL,
    name text NOT NULL,
    lat double precision NOT NULL,
    lon double precision NOT NULL,
    radius_m real,
    focus boolean DEFAULT false NOT NULL,
    created_by text,
    updated_at timestamp with time zone DEFAULT now() NOT NULL,
    CONSTRAINT zones_radius_m_check CHECK (((radius_m >= (30)::double precision) AND (radius_m <= (1000)::double precision)))
);

CREATE TABLE ops.audit_log (
    id bigint NOT NULL,
    at timestamp with time zone DEFAULT now() NOT NULL,
    entity text NOT NULL,
    event text NOT NULL,
    by_label text NOT NULL,
    payload jsonb
);

CREATE SEQUENCE ops.audit_log_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE ops.audit_log_id_seq OWNED BY ops.audit_log.id;

CREATE TABLE ops.cell_events (
    id bigint NOT NULL,
    shard text NOT NULL,
    from_cell text NOT NULL,
    kind text NOT NULL,
    a bytea,
    b bytea,
    req bigint,
    at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE SEQUENCE ops.cell_events_id_seq
    START WITH 1
    INCREMENT BY 1
    NO MINVALUE
    NO MAXVALUE
    CACHE 1;

ALTER SEQUENCE ops.cell_events_id_seq OWNED BY ops.cell_events.id;

CREATE TABLE ops.cells (
    shard text NOT NULL,
    cell text NOT NULL,
    upstream text NOT NULL
);

CREATE TABLE ops.data_versions (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    version_type text NOT NULL,
    label text,
    source text,
    notes text,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE ops.import_jobs (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    job_type text NOT NULL,
    region_id uuid,
    status text DEFAULT 'pending'::text NOT NULL,
    started_at timestamp with time zone,
    finished_at timestamp with time zone,
    metadata_json jsonb,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE ops.region_lease (
    slug text NOT NULL,
    cell text NOT NULL,
    until timestamp with time zone NOT NULL
);

CREATE TABLE ops.tokens (
    hash bytea NOT NULL,
    region text,
    label text NOT NULL,
    created date DEFAULT CURRENT_DATE NOT NULL,
    role text DEFAULT 'admin'::text NOT NULL,
    CONSTRAINT tokens_hash_check CHECK ((length(hash) = 32)),
    CONSTRAINT tokens_role_check CHECK ((role = ANY (ARRAY['read'::text, 'editor'::text, 'admin'::text])))
);

CREATE TABLE ops.used_once (
    k text NOT NULL,
    exp timestamp with time zone NOT NULL
);

CREATE TABLE terrain.dem_tiles (
    id uuid DEFAULT gen_random_uuid() NOT NULL,
    region_id uuid NOT NULL,
    tile_name text NOT NULL,
    raster_ref text,
    resolution_m double precision,
    vertical_datum text,
    source text,
    import_job_id uuid,
    created_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE terrain.edge_samples (
    edge_id bigint NOT NULL,
    elev_from_m double precision NOT NULL,
    elev_to_m double precision NOT NULL,
    delta_elevation_m double precision,
    grade_forward double precision,
    grade_reverse double precision,
    slope_class text,
    sample_source text,
    sampled_at timestamp with time zone DEFAULT now() NOT NULL
);

CREATE TABLE terrain.node_samples (
    node_id bigint NOT NULL,
    elevation_m double precision NOT NULL,
    slope_deg double precision,
    aspect_deg double precision,
    roughness double precision,
    sample_source text,
    sample_confidence double precision DEFAULT 1.0 NOT NULL,
    sampled_at timestamp with time zone DEFAULT now() NOT NULL
);

ALTER TABLE ONLY analytics.interventions ALTER COLUMN id SET DEFAULT nextval('analytics.interventions_id_seq'::regclass);

ALTER TABLE ONLY game.feedback ALTER COLUMN id SET DEFAULT nextval('game.feedback_id_seq'::regclass);

ALTER TABLE ONLY game.hotspot_plans ALTER COLUMN id SET DEFAULT nextval('game.hotspot_plans_id_seq'::regclass);

ALTER TABLE ONLY game.shard_outbox ALTER COLUMN id SET DEFAULT nextval('game.shard_outbox_id_seq'::regclass);

ALTER TABLE ONLY geo.finds ALTER COLUMN id SET DEFAULT nextval('geo.finds_id_seq'::regclass);

ALTER TABLE ONLY geo.hotspot_sites ALTER COLUMN id SET DEFAULT nextval('geo.hotspot_sites_id_seq'::regclass);

ALTER TABLE ONLY geo.media ALTER COLUMN id SET DEFAULT nextval('geo.media_id_seq'::regclass);

ALTER TABLE ONLY ops.audit_log ALTER COLUMN id SET DEFAULT nextval('ops.audit_log_id_seq'::regclass);

ALTER TABLE ONLY ops.cell_events ALTER COLUMN id SET DEFAULT nextval('ops.cell_events_id_seq'::regclass);

ALTER TABLE ONLY analytics.area_hour
    ADD CONSTRAINT area_hour_pkey PRIMARY KEY (region, hour_start);

ALTER TABLE ONLY analytics.block_day
    ADD CONSTRAINT block_day_pkey PRIMARY KEY (region, day, block_id);

ALTER TABLE ONLY analytics.daily
    ADD CONSTRAINT daily_pkey PRIMARY KEY (region, day, synthetic);

ALTER TABLE ONLY analytics.daily_profile
    ADD CONSTRAINT daily_profile_pkey PRIMARY KEY (region, day, synthetic, age, local);

ALTER TABLE ONLY analytics.dwell
    ADD CONSTRAINT dwell_pkey PRIMARY KEY (region, edge_id, window_start, synthetic, span_s);

ALTER TABLE ONLY analytics.flow_day
    ADD CONSTRAINT flow_day_pkey PRIMARY KEY (region, from_zone, to_zone, day, synthetic);

ALTER TABLE ONLY analytics.flow
    ADD CONSTRAINT flow_pkey PRIMARY KEY (region, from_zone, to_zone, hour_start, synthetic);

ALTER TABLE ONLY analytics.growth_day
    ADD CONSTRAINT growth_day_pkey PRIMARY KEY (region, day);

ALTER TABLE ONLY analytics.hex_dwell
    ADD CONSTRAINT hex_dwell_pkey PRIMARY KEY (region, res, cell, hour_start, synthetic, span_s);

ALTER TABLE ONLY analytics.hex_suppressed
    ADD CONSTRAINT hex_suppressed_pkey PRIMARY KEY (region, res, hour_start, synthetic);

ALTER TABLE ONLY analytics.interventions
    ADD CONSTRAINT interventions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY analytics.link_day
    ADD CONSTRAINT link_day_pkey PRIMARY KEY (day, code);

ALTER TABLE ONLY analytics.perception
    ADD CONSTRAINT perception_pkey PRIMARY KEY (region, node_id, day, hour, category, synthetic);

ALTER TABLE ONLY analytics.place_visits
    ADD CONSTRAINT place_visits_pkey PRIMARY KEY (region, zone_id, start, span_s, synthetic);

ALTER TABLE ONLY analytics.site_visits
    ADD CONSTRAINT site_visits_pkey PRIMARY KEY (region, site_node, slot_start, synthetic);

ALTER TABLE ONLY analytics.suppressed
    ADD CONSTRAINT suppressed_pkey PRIMARY KEY (region, window_start, synthetic);

ALTER TABLE ONLY core.factions
    ADD CONSTRAINT factions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY core.factions
    ADD CONSTRAINT factions_slug_key UNIQUE (slug);

ALTER TABLE ONLY game.campaign_mottos
    ADD CONSTRAINT campaign_mottos_pkey PRIMARY KEY (region, num);

ALTER TABLE ONLY game.chapter_progress
    ADD CONSTRAINT chapter_progress_pkey PRIMARY KEY (region, key, faction);

ALTER TABLE ONLY game.chapters
    ADD CONSTRAINT chapters_pkey PRIMARY KEY (region, key);

ALTER TABLE ONLY game.faction_day
    ADD CONSTRAINT faction_day_pkey PRIMARY KEY (region, day, faction);

ALTER TABLE ONLY game.feedback
    ADD CONSTRAINT feedback_pkey PRIMARY KEY (id);

ALTER TABLE ONLY game.hotspot_plans
    ADD CONSTRAINT hotspot_plans_pkey PRIMARY KEY (id);

ALTER TABLE ONLY game.keep_pot
    ADD CONSTRAINT keep_pot_pkey PRIMARY KEY (region, ticket);

ALTER TABLE ONLY game.keep_resets
    ADD CONSTRAINT keep_resets_pkey PRIMARY KEY (region);

ALTER TABLE ONLY game.keep_slots
    ADD CONSTRAINT keep_slots_pkey PRIMARY KEY (region, slot);

ALTER TABLE ONLY game.keeps
    ADD CONSTRAINT keeps_pkey PRIMARY KEY (region, node_id);

ALTER TABLE ONLY game.keeps
    ADD CONSTRAINT keeps_ticket_key UNIQUE (ticket);

ALTER TABLE ONLY game.player_badges
    ADD CONSTRAINT player_badges_pkey PRIMARY KEY (id, badge);

ALTER TABLE ONLY game.player_finds
    ADD CONSTRAINT player_finds_pkey PRIMARY KEY (id, find);

ALTER TABLE ONLY game.player_presence
    ADD CONSTRAINT player_presence_pkey PRIMARY KEY (id, code);

ALTER TABLE ONLY game.player_region
    ADD CONSTRAINT player_region_pkey PRIMARY KEY (region, id);

ALTER TABLE ONLY game.players
    ADD CONSTRAINT players_pkey PRIMARY KEY (id);

ALTER TABLE ONLY game.presence_codes
    ADD CONSTRAINT presence_codes_pkey PRIMARY KEY (code);

ALTER TABLE ONLY game.shard_outbox
    ADD CONSTRAINT shard_outbox_pkey PRIMARY KEY (id);

ALTER TABLE ONLY game.weather_days
    ADD CONSTRAINT weather_days_pkey PRIMARY KEY (region, day);

ALTER TABLE ONLY game.wishes
    ADD CONSTRAINT wishes_pkey PRIMARY KEY (cell, day);

ALTER TABLE ONLY geo.blocks
    ADD CONSTRAINT blocks_pkey PRIMARY KEY (region, id);

ALTER TABLE ONLY geo.commons_suggestions
    ADD CONSTRAINT commons_suggestions_pkey PRIMARY KEY (find, title);

ALTER TABLE ONLY geo.edges
    ADD CONSTRAINT edges_pkey PRIMARY KEY (id);

ALTER TABLE ONLY geo.finds
    ADD CONSTRAINT finds_pkey PRIMARY KEY (id);

ALTER TABLE ONLY geo.finds
    ADD CONSTRAINT finds_region_osm_key UNIQUE (region, osm);

ALTER TABLE ONLY geo.hotspot_sites
    ADD CONSTRAINT hotspot_sites_pkey PRIMARY KEY (id);

ALTER TABLE ONLY geo.hotspot_sites
    ADD CONSTRAINT hotspot_sites_region_node_id_key UNIQUE (region, node_id);

ALTER TABLE ONLY geo.media
    ADD CONSTRAINT media_pkey PRIMARY KEY (id);

ALTER TABLE ONLY geo.media
    ADD CONSTRAINT media_s3_key_key UNIQUE (s3_key);

ALTER TABLE ONLY geo.node_tide
    ADD CONSTRAINT node_tide_pkey PRIMARY KEY (node_id);

ALTER TABLE ONLY geo.nodes
    ADD CONSTRAINT nodes_pkey PRIMARY KEY (id);

ALTER TABLE ONLY geo.region_tide
    ADD CONSTRAINT region_tide_pkey PRIMARY KEY (region);

ALTER TABLE ONLY geo.regions
    ADD CONSTRAINT regions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY geo.regions
    ADD CONSTRAINT regions_slug_key UNIQUE (slug);

ALTER TABLE ONLY geo.territories
    ADD CONSTRAINT territories_pkey PRIMARY KEY (id);

ALTER TABLE ONLY geo.zones
    ADD CONSTRAINT zones_pkey PRIMARY KEY (region, id);

ALTER TABLE ONLY ops.audit_log
    ADD CONSTRAINT audit_log_pkey PRIMARY KEY (id);

ALTER TABLE ONLY ops.cell_events
    ADD CONSTRAINT cell_events_pkey PRIMARY KEY (id);

ALTER TABLE ONLY ops.cells
    ADD CONSTRAINT cells_pkey PRIMARY KEY (shard, cell);

ALTER TABLE ONLY ops.data_versions
    ADD CONSTRAINT data_versions_pkey PRIMARY KEY (id);

ALTER TABLE ONLY ops.import_jobs
    ADD CONSTRAINT import_jobs_pkey PRIMARY KEY (id);

ALTER TABLE ONLY ops.region_lease
    ADD CONSTRAINT region_lease_pkey PRIMARY KEY (slug);

ALTER TABLE ONLY ops.tokens
    ADD CONSTRAINT tokens_label_key UNIQUE (label);

ALTER TABLE ONLY ops.tokens
    ADD CONSTRAINT tokens_pkey PRIMARY KEY (hash);

ALTER TABLE ONLY ops.used_once
    ADD CONSTRAINT used_once_pkey PRIMARY KEY (k);

ALTER TABLE ONLY terrain.dem_tiles
    ADD CONSTRAINT dem_tiles_pkey PRIMARY KEY (id);

ALTER TABLE ONLY terrain.edge_samples
    ADD CONSTRAINT edge_samples_pkey PRIMARY KEY (edge_id);

ALTER TABLE ONLY terrain.node_samples
    ADD CONSTRAINT node_samples_pkey PRIMARY KEY (node_id);

CREATE INDEX dwell_window_idx ON analytics.dwell USING btree (region, window_start);

CREATE INDEX flow_hour_idx ON analytics.flow USING btree (region, hour_start);

CREATE INDEX site_visits_slot_idx ON analytics.site_visits USING btree (region, slot_start);

CREATE INDEX feedback_day ON game.feedback USING btree (day);

CREATE INDEX hotspot_plans_region_ends ON game.hotspot_plans USING btree (region, ends);

CREATE INDEX keep_slots_player ON game.keep_slots USING btree (region, id);

CREATE INDEX player_region_id_idx ON game.player_region USING btree (id);

CREATE UNIQUE INDEX players_home_idx ON game.players USING btree (home, home_id) WHERE (home IS NOT NULL);

CREATE UNIQUE INDEX players_recovery_idx ON game.players USING btree (recovery_hash) WHERE (recovery_hash IS NOT NULL);

CREATE INDEX players_region_idx ON game.players USING btree (region);

CREATE INDEX blocks_geom ON geo.blocks USING gist (geom);

CREATE INDEX commons_suggestions_region ON geo.commons_suggestions USING btree (region);

CREATE INDEX hotspot_sites_region ON geo.hotspot_sites USING btree (region);

CREATE INDEX idx_geo_edges_active ON geo.edges USING btree (region_id) WHERE (is_active = true);

CREATE INDEX idx_geo_edges_geom ON geo.edges USING gist (geom);

CREATE INDEX idx_geo_edges_region ON geo.edges USING btree (region_id);

CREATE INDEX idx_geo_edges_region_source ON geo.edges USING btree (region_id, source);

CREATE INDEX idx_geo_edges_region_target ON geo.edges USING btree (region_id, target);

CREATE INDEX idx_geo_edges_source ON geo.edges USING btree (source);

CREATE INDEX idx_geo_edges_target ON geo.edges USING btree (target);

CREATE INDEX idx_geo_nodes_active ON geo.nodes USING btree (region_id) WHERE (is_active = true);

CREATE INDEX idx_geo_nodes_geom ON geo.nodes USING gist (geom);

CREATE INDEX idx_geo_nodes_region ON geo.nodes USING btree (region_id);

CREATE INDEX idx_geo_regions_boundary ON geo.regions USING gist (boundary_geom);

CREATE INDEX idx_geo_regions_centroid ON geo.regions USING gist (centroid_geom);

CREATE INDEX idx_geo_territories_polygon ON geo.territories USING gist (polygon_geom);

CREATE INDEX idx_geo_territories_region ON geo.territories USING btree (region_id);

CREATE UNIQUE INDEX media_one_approved ON geo.media USING btree (region, kind, target) WHERE (status = 'approved'::text);

CREATE INDEX media_region ON geo.media USING btree (region, kind, target);

CREATE INDEX cell_events_shard_id ON ops.cell_events USING btree (shard, id);

ALTER TABLE ONLY game.campaign_mottos
    ADD CONSTRAINT campaign_mottos_region_fkey FOREIGN KEY (region) REFERENCES geo.regions(slug) ON DELETE CASCADE;

ALTER TABLE ONLY game.chapter_progress
    ADD CONSTRAINT chapter_progress_region_key_fkey FOREIGN KEY (region, key) REFERENCES game.chapters(region, key) ON DELETE CASCADE;

ALTER TABLE ONLY game.hotspot_plans
    ADD CONSTRAINT hotspot_plans_region_fkey FOREIGN KEY (region) REFERENCES geo.regions(slug) ON DELETE CASCADE;

ALTER TABLE ONLY game.keep_slots
    ADD CONSTRAINT keep_slots_id_fkey FOREIGN KEY (id) REFERENCES game.players(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY game.player_badges
    ADD CONSTRAINT player_badges_id_fkey FOREIGN KEY (id) REFERENCES game.players(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY game.player_finds
    ADD CONSTRAINT player_finds_find_fkey FOREIGN KEY (find) REFERENCES geo.finds(id) ON DELETE CASCADE;

ALTER TABLE ONLY game.player_finds
    ADD CONSTRAINT player_finds_id_fkey FOREIGN KEY (id) REFERENCES game.players(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY game.player_presence
    ADD CONSTRAINT player_presence_code_fkey FOREIGN KEY (code) REFERENCES game.presence_codes(code) ON DELETE CASCADE;

ALTER TABLE ONLY game.player_presence
    ADD CONSTRAINT player_presence_id_fkey FOREIGN KEY (id) REFERENCES game.players(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY game.player_region
    ADD CONSTRAINT player_region_id_fkey FOREIGN KEY (id) REFERENCES game.players(id) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY geo.commons_suggestions
    ADD CONSTRAINT commons_suggestions_find_fkey FOREIGN KEY (find) REFERENCES geo.finds(id) ON DELETE CASCADE;

ALTER TABLE ONLY geo.commons_suggestions
    ADD CONSTRAINT commons_suggestions_region_fkey FOREIGN KEY (region) REFERENCES geo.regions(slug) ON DELETE CASCADE;

ALTER TABLE ONLY geo.edges
    ADD CONSTRAINT edges_region_id_fkey FOREIGN KEY (region_id) REFERENCES geo.regions(id);

ALTER TABLE ONLY geo.edges
    ADD CONSTRAINT edges_source_fkey FOREIGN KEY (source) REFERENCES geo.nodes(id);

ALTER TABLE ONLY geo.edges
    ADD CONSTRAINT edges_target_fkey FOREIGN KEY (target) REFERENCES geo.nodes(id);

ALTER TABLE ONLY geo.hotspot_sites
    ADD CONSTRAINT hotspot_sites_region_fkey FOREIGN KEY (region) REFERENCES geo.regions(slug) ON DELETE CASCADE;

ALTER TABLE ONLY geo.media
    ADD CONSTRAINT media_region_fkey FOREIGN KEY (region) REFERENCES geo.regions(slug) ON DELETE CASCADE;

ALTER TABLE ONLY geo.node_tide
    ADD CONSTRAINT node_tide_node_id_fkey FOREIGN KEY (node_id) REFERENCES geo.nodes(id) ON DELETE CASCADE;

ALTER TABLE ONLY geo.nodes
    ADD CONSTRAINT nodes_owner_faction_id_fkey FOREIGN KEY (owner_faction_id) REFERENCES core.factions(id);

ALTER TABLE ONLY geo.nodes
    ADD CONSTRAINT nodes_region_id_fkey FOREIGN KEY (region_id) REFERENCES geo.regions(id);

ALTER TABLE ONLY geo.region_tide
    ADD CONSTRAINT region_tide_region_fkey FOREIGN KEY (region) REFERENCES geo.regions(slug) ON UPDATE CASCADE ON DELETE CASCADE;

ALTER TABLE ONLY geo.regions
    ADD CONSTRAINT regions_road_graph_version_id_fkey FOREIGN KEY (road_graph_version_id) REFERENCES ops.data_versions(id);

ALTER TABLE ONLY geo.regions
    ADD CONSTRAINT regions_ruleset_version_id_fkey FOREIGN KEY (ruleset_version_id) REFERENCES ops.data_versions(id);

ALTER TABLE ONLY geo.regions
    ADD CONSTRAINT regions_terrain_version_id_fkey FOREIGN KEY (terrain_version_id) REFERENCES ops.data_versions(id);

ALTER TABLE ONLY geo.territories
    ADD CONSTRAINT territories_derived_from_version_fkey FOREIGN KEY (derived_from_version) REFERENCES ops.data_versions(id);

ALTER TABLE ONLY geo.territories
    ADD CONSTRAINT territories_owner_faction_id_fkey FOREIGN KEY (owner_faction_id) REFERENCES core.factions(id);

ALTER TABLE ONLY geo.territories
    ADD CONSTRAINT territories_region_id_fkey FOREIGN KEY (region_id) REFERENCES geo.regions(id);

ALTER TABLE ONLY terrain.dem_tiles
    ADD CONSTRAINT dem_tiles_import_job_id_fkey FOREIGN KEY (import_job_id) REFERENCES ops.import_jobs(id);

ALTER TABLE ONLY terrain.dem_tiles
    ADD CONSTRAINT dem_tiles_region_id_fkey FOREIGN KEY (region_id) REFERENCES geo.regions(id);

ALTER TABLE ONLY terrain.edge_samples
    ADD CONSTRAINT edge_samples_edge_id_fkey FOREIGN KEY (edge_id) REFERENCES geo.edges(id);

ALTER TABLE ONLY terrain.node_samples
    ADD CONSTRAINT node_samples_node_id_fkey FOREIGN KEY (node_id) REFERENCES geo.nodes(id);

ALTER TABLE core.factions ENABLE ROW LEVEL SECURITY;

CREATE POLICY factions_select ON core.factions FOR SELECT USING (true);

ALTER TABLE geo.edges ENABLE ROW LEVEL SECURITY;

CREATE POLICY edges_select ON geo.edges FOR SELECT USING (true);

ALTER TABLE geo.nodes ENABLE ROW LEVEL SECURITY;

CREATE POLICY nodes_select ON geo.nodes FOR SELECT USING (true);

ALTER TABLE geo.regions ENABLE ROW LEVEL SECURITY;

CREATE POLICY regions_select ON geo.regions FOR SELECT USING (true);

ALTER TABLE geo.territories ENABLE ROW LEVEL SECURITY;

CREATE POLICY territories_select ON geo.territories FOR SELECT USING (true);

ALTER TABLE ops.data_versions ENABLE ROW LEVEL SECURITY;

ALTER TABLE ops.import_jobs ENABLE ROW LEVEL SECURITY;

ALTER TABLE terrain.dem_tiles ENABLE ROW LEVEL SECURITY;

CREATE POLICY dem_tiles_select ON terrain.dem_tiles FOR SELECT USING (true);

ALTER TABLE terrain.edge_samples ENABLE ROW LEVEL SECURITY;

CREATE POLICY edge_samples_select ON terrain.edge_samples FOR SELECT USING (true);

ALTER TABLE terrain.node_samples ENABLE ROW LEVEL SECURITY;

CREATE POLICY node_samples_select ON terrain.node_samples FOR SELECT USING (true);

-- Privileges of the roles above.
DO $$
BEGIN
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
    GRANT USAGE ON SCHEMA analytics TO elements_app;
    GRANT USAGE ON SCHEMA game TO elements_app;
    GRANT USAGE ON SCHEMA geo TO elements_app;
    GRANT USAGE ON SCHEMA ops TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.area_hour TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.block_day TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.daily TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.daily_profile TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.dwell TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.flow TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.flow_day TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.growth_day TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.hex_dwell TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.hex_suppressed TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.interventions TO elements_app;
    GRANT SELECT,USAGE ON SEQUENCE analytics.interventions_id_seq TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.link_day TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.perception TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.place_visits TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.site_visits TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE analytics.suppressed TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.campaign_mottos TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.chapter_progress TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.chapters TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.faction_day TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.feedback TO elements_app;
    GRANT SELECT,USAGE ON SEQUENCE game.feedback_id_seq TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.hotspot_plans TO elements_app;
    GRANT SELECT,USAGE ON SEQUENCE game.hotspot_plans_id_seq TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.keep_pot TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.keep_resets TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.keep_slots TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.keeps TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.player_badges TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.player_finds TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.player_presence TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.player_region TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.players TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.presence_codes TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.shard_outbox TO elements_app;
    GRANT SELECT,USAGE ON SEQUENCE game.shard_outbox_id_seq TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.weather_days TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE game.wishes TO elements_app;
    GRANT SELECT ON TABLE geo.blocks TO elements_app;
    GRANT SELECT ON TABLE geo.commons_suggestions TO elements_app;
    GRANT SELECT ON TABLE geo.edges TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE geo.finds TO elements_app;
    GRANT SELECT,USAGE ON SEQUENCE geo.finds_id_seq TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE geo.hotspot_sites TO elements_app;
    GRANT SELECT,USAGE ON SEQUENCE geo.hotspot_sites_id_seq TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE geo.media TO elements_app;
    GRANT SELECT,USAGE ON SEQUENCE geo.media_id_seq TO elements_app;
    GRANT SELECT ON TABLE geo.node_tide TO elements_app;
    GRANT SELECT ON TABLE geo.nodes TO elements_app;
    GRANT SELECT ON TABLE geo.region_tide TO elements_app;
    GRANT SELECT ON TABLE geo.regions TO elements_app;
    GRANT SELECT ON TABLE geo.territories TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE geo.zones TO elements_app;
    GRANT SELECT,INSERT ON TABLE ops.audit_log TO elements_app;
    GRANT USAGE ON SEQUENCE ops.audit_log_id_seq TO elements_app;
    GRANT SELECT,INSERT,DELETE ON TABLE ops.cell_events TO elements_app;
    GRANT USAGE ON SEQUENCE ops.cell_events_id_seq TO elements_app;
    GRANT SELECT ON TABLE ops.cells TO elements_app;
    GRANT SELECT,INSERT,DELETE,UPDATE ON TABLE ops.region_lease TO elements_app;
    GRANT SELECT,INSERT,DELETE ON TABLE ops.tokens TO elements_app;
    GRANT SELECT,INSERT,DELETE ON TABLE ops.used_once TO elements_app;
    -- Gastronomy near hotspot sites (editor): from db/import/osm-views.sql.
    IF to_regclass('osm.pois_all') IS NOT NULL THEN
      GRANT USAGE ON SCHEMA osm TO elements_app;
      GRANT SELECT ON osm.pois_all TO elements_app;
    END IF;
  END IF;
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_read') THEN
    GRANT USAGE ON SCHEMA analytics TO elements_read;
    GRANT USAGE ON SCHEMA geo TO elements_read;
    GRANT SELECT ON TABLE analytics.area_hour TO elements_read;
    GRANT SELECT ON TABLE analytics.block_day TO elements_read;
    GRANT SELECT ON TABLE analytics.daily TO elements_read;
    GRANT SELECT ON TABLE analytics.daily_profile TO elements_read;
    GRANT SELECT ON TABLE analytics.dwell TO elements_read;
    GRANT SELECT ON TABLE analytics.flow TO elements_read;
    GRANT SELECT ON TABLE analytics.flow_day TO elements_read;
    GRANT SELECT ON TABLE analytics.growth_day TO elements_read;
    GRANT SELECT ON TABLE analytics.hex_dwell TO elements_read;
    GRANT SELECT ON TABLE analytics.hex_suppressed TO elements_read;
    GRANT SELECT ON TABLE analytics.interventions TO elements_read;
    GRANT SELECT ON TABLE analytics.link_day TO elements_read;
    GRANT SELECT ON TABLE analytics.perception TO elements_read;
    GRANT SELECT ON TABLE analytics.place_visits TO elements_read;
    GRANT SELECT ON TABLE analytics.site_visits TO elements_read;
    GRANT SELECT ON TABLE analytics.suppressed TO elements_read;
    GRANT SELECT ON TABLE geo.blocks TO elements_read;
    GRANT SELECT ON TABLE geo.commons_suggestions TO elements_read;
    GRANT SELECT ON TABLE geo.edges TO elements_read;
    GRANT SELECT ON TABLE geo.finds TO elements_read;
    GRANT SELECT ON TABLE geo.hotspot_sites TO elements_read;
    GRANT SELECT ON TABLE geo.media TO elements_read;
    GRANT SELECT ON TABLE geo.node_tide TO elements_read;
    GRANT SELECT ON TABLE geo.nodes TO elements_read;
    GRANT SELECT ON TABLE geo.region_tide TO elements_read;
    GRANT SELECT ON TABLE geo.regions TO elements_read;
    GRANT SELECT ON TABLE geo.territories TO elements_read;
    GRANT SELECT ON TABLE geo.zones TO elements_read;
    GRANT SELECT ON TABLE ops.audit_log TO elements_read;
  END IF;
  IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_owner') THEN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_app') THEN
      ALTER DEFAULT PRIVILEGES FOR ROLE elements_owner IN SCHEMA analytics GRANT SELECT,USAGE ON SEQUENCES TO elements_app;
      ALTER DEFAULT PRIVILEGES FOR ROLE elements_owner IN SCHEMA analytics GRANT SELECT,INSERT,DELETE,UPDATE ON TABLES TO elements_app;
      ALTER DEFAULT PRIVILEGES FOR ROLE elements_owner IN SCHEMA game GRANT SELECT,USAGE ON SEQUENCES TO elements_app;
      ALTER DEFAULT PRIVILEGES FOR ROLE elements_owner IN SCHEMA game GRANT SELECT,INSERT,DELETE,UPDATE ON TABLES TO elements_app;
      ALTER DEFAULT PRIVILEGES FOR ROLE elements_owner IN SCHEMA geo GRANT SELECT ON SEQUENCES TO elements_app;
      ALTER DEFAULT PRIVILEGES FOR ROLE elements_owner IN SCHEMA geo GRANT SELECT ON TABLES TO elements_app;
    END IF;
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'elements_read') THEN
      ALTER DEFAULT PRIVILEGES FOR ROLE elements_owner IN SCHEMA analytics GRANT SELECT ON TABLES TO elements_read;
      ALTER DEFAULT PRIVILEGES FOR ROLE elements_owner IN SCHEMA geo GRANT SELECT ON TABLES TO elements_read;
    END IF;
  END IF;
END $$;
