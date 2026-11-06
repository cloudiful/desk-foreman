INSERT INTO applications (name) VALUES ($1) RETURNING application_id;
