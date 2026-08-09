CREATE TABLE roles (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    name_key TEXT NOT NULL
);

CREATE UNIQUE INDEX roles_name_key_idx ON roles (name_key);

CREATE TABLE score_person_roles (
    score_id INTEGER NOT NULL,
    person_id INTEGER NOT NULL,
    role_id INTEGER NOT NULL REFERENCES roles (id) ON DELETE CASCADE,
    PRIMARY KEY (score_id, person_id, role_id),
    FOREIGN KEY (score_id, person_id)
        REFERENCES score_people (score_id, person_id) ON DELETE CASCADE
);

CREATE INDEX score_person_roles_role_id_idx ON score_person_roles (role_id);
