-- AD-E1 enterprise request-principal lifecycle.
-- Additive only. These tables contain identity bindings and lifecycle state,
-- never bearer/OIDC tokens, secret material, device keys, or policy authority.

CREATE TABLE enterprise_principals (
    id                            TEXT PRIMARY KEY
                                  CHECK (
                                      length(id) = 29
                                      AND substr(id, 1, 3) = 'ep_'
                                      AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                                  ),
    organization_authority_key    TEXT NOT NULL CHECK (length(trim(organization_authority_key)) > 0),
    organization_resource_id      TEXT NOT NULL CHECK (length(trim(organization_resource_id)) > 0),
    organization_resource_version TEXT NOT NULL CHECK (length(trim(organization_resource_version)) > 0),
    kind                          TEXT NOT NULL CHECK (kind IN ('human', 'service')),
    status                        TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    display_name                  TEXT NOT NULL CHECK (length(trim(display_name)) > 0),
    created_at                    INTEGER NOT NULL,
    updated_at                    INTEGER NOT NULL,
    disabled_at                   INTEGER,
    CHECK (
        (status = 'active' AND disabled_at IS NULL)
        OR (status = 'disabled' AND disabled_at IS NOT NULL)
    )
);

CREATE INDEX idx_enterprise_principals_organization
    ON enterprise_principals(
        organization_authority_key,
        organization_resource_id,
        organization_resource_version,
        status,
        id
    );

CREATE TABLE oidc_principal_bindings (
    issuer       TEXT NOT NULL CHECK (length(trim(issuer)) > 0),
    subject      TEXT NOT NULL CHECK (length(trim(subject)) > 0),
    principal_id TEXT NOT NULL REFERENCES enterprise_principals(id) ON DELETE RESTRICT,
    bound_at     INTEGER NOT NULL,
    PRIMARY KEY (issuer, subject)
);

CREATE INDEX idx_oidc_principal_bindings_principal
    ON oidc_principal_bindings(principal_id, issuer, subject);

CREATE TABLE matrix_principal_users (
    user_id      TEXT PRIMARY KEY CHECK (length(trim(user_id)) > 0),
    homeserver   TEXT NOT NULL CHECK (length(trim(homeserver)) > 0),
    principal_id TEXT NOT NULL REFERENCES enterprise_principals(id) ON DELETE RESTRICT,
    status       TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    bound_at     INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    disabled_at  INTEGER,
    CHECK (
        (status = 'active' AND disabled_at IS NULL)
        OR (status = 'disabled' AND disabled_at IS NOT NULL)
    )
);

CREATE INDEX idx_matrix_principal_users_principal
    ON matrix_principal_users(principal_id, status, user_id);

CREATE TABLE matrix_principal_devices (
    user_id      TEXT NOT NULL REFERENCES matrix_principal_users(user_id) ON DELETE RESTRICT,
    device_id    TEXT NOT NULL CHECK (length(trim(device_id)) > 0),
    principal_id TEXT NOT NULL REFERENCES enterprise_principals(id) ON DELETE RESTRICT,
    status       TEXT NOT NULL CHECK (status IN ('current', 'revoked')),
    bound_at     INTEGER NOT NULL,
    revoked_at   INTEGER,
    PRIMARY KEY (user_id, device_id),
    CHECK (
        (status = 'current' AND revoked_at IS NULL)
        OR (status = 'revoked' AND revoked_at IS NOT NULL)
    )
);

CREATE INDEX idx_matrix_principal_devices_principal
    ON matrix_principal_devices(principal_id, status, user_id, device_id);

CREATE TABLE matrix_principal_appservices (
    appservice_id           TEXT NOT NULL CHECK (length(trim(appservice_id)) > 0),
    homeserver              TEXT NOT NULL CHECK (length(trim(homeserver)) > 0),
    sender_localpart_prefix TEXT NOT NULL CHECK (length(trim(sender_localpart_prefix)) > 0),
    principal_id            TEXT NOT NULL REFERENCES enterprise_principals(id) ON DELETE RESTRICT,
    status                  TEXT NOT NULL CHECK (status IN ('active', 'disabled')),
    bound_at                INTEGER NOT NULL,
    disabled_at             INTEGER,
    PRIMARY KEY (appservice_id, homeserver),
    CHECK (
        (status = 'active' AND disabled_at IS NULL)
        OR (status = 'disabled' AND disabled_at IS NOT NULL)
    )
);

CREATE INDEX idx_matrix_principal_appservices_principal
    ON matrix_principal_appservices(principal_id, status, appservice_id, homeserver);

UPDATE schema_meta SET value = '17' WHERE key = 'version';
