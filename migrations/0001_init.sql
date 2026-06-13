-- APEX backend schema. All Move u64 values fit comfortably in BIGINT for this
-- protocol's domain (dUSDC base units, ms timestamps); u128 weights are TEXT.

CREATE TABLE indexer_cursors (
    filter_key  TEXT PRIMARY KEY,            -- 'apex_pool' | 'reputation'
    tx_digest   TEXT NOT NULL,
    event_seq   TEXT NOT NULL,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE pools (
    pool_id                TEXT PRIMARY KEY,
    leg_count              BIGINT NOT NULL DEFAULT 1,
    state                  TEXT NOT NULL DEFAULT 'Open',
    start_time_ms          BIGINT,
    commit_deadline_ms     BIGINT NOT NULL,
    reveal_deadline_ms     BIGINT NOT NULL,
    oracle_expiry_ms       BIGINT,
    entry_fee_amount       BIGINT NOT NULL DEFAULT 0,
    oracle_ids             JSONB,
    oracle_results         JSONB,
    participant_count      BIGINT NOT NULL DEFAULT 0,
    reveal_count           BIGINT NOT NULL DEFAULT 0,
    forfeited_count        BIGINT NOT NULL DEFAULT 0,
    prize_pool             BIGINT NOT NULL DEFAULT 0,
    total_weight           TEXT NOT NULL DEFAULT '0',
    total_prize            BIGINT,
    initial_shared_version BIGINT,
    predictions_table_id   TEXT,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_pools_state ON pools(state);
CREATE INDEX idx_pools_commit_deadline ON pools(commit_deadline_ms);

CREATE TABLE participants (
    pool_id               TEXT NOT NULL REFERENCES pools(pool_id),
    participant           TEXT NOT NULL,
    entry_fee             BIGINT NOT NULL DEFAULT 0,
    commit_timestamp_ms   BIGINT NOT NULL DEFAULT 0,
    revealed              BOOLEAN NOT NULL DEFAULT FALSE,
    crowd_median_estimate BIGINT,
    predicted_prices      JSONB,
    leg_hits              SMALLINT,
    acc_score             BIGINT,
    time_bonus            BIGINT,
    composite_weight      TEXT,
    apex_payout           BIGINT,
    paid                  BOOLEAN NOT NULL DEFAULT FALSE,
    PRIMARY KEY (pool_id, participant)
);
CREATE INDEX idx_participants_addr ON participants(participant);

CREATE TABLE frs_profiles (
    participant           TEXT PRIMARY KEY,
    tier                  SMALLINT NOT NULL DEFAULT 0,
    lifetime_accuracy_bps BIGINT NOT NULL DEFAULT 0,
    pool_count            BIGINT NOT NULL DEFAULT 0,
    frs_pool_id           TEXT,
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE frs_settlements (
    participant           TEXT NOT NULL,
    pool_id               TEXT NOT NULL,
    new_tier              SMALLINT NOT NULL,
    lifetime_accuracy_bps BIGINT NOT NULL,
    pool_count            BIGINT NOT NULL,
    tx_digest             TEXT NOT NULL,
    settled_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (participant, pool_id)
);
CREATE INDEX idx_frs_settlements_participant ON frs_settlements(participant, pool_count);

CREATE TABLE keeper_actions (
    id         BIGSERIAL PRIMARY KEY,
    pool_id    TEXT NOT NULL,
    action     TEXT NOT NULL,               -- close|lock|resolve|distribute
    tx_digest  TEXT,
    status     TEXT NOT NULL,               -- sent|success|failed
    error      TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX idx_keeper_actions_pool ON keeper_actions(pool_id, action, created_at DESC);
