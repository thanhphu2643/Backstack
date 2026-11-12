#pragma once

// Every function takes a UTF-8 JSON string and returns a newly allocated JSON string
// (or {"error": "..."}) that must be released with backstack_free.
// A target is {via: "docker", container, user} or
// {via: "server", host, port, user, password?, sslmode?, allow_drop?}.
// An S3 config is {endpoint, port?, use_ssl, region?, bucket, prefix?, access_key, secret_key}.

/// {folders, buckets: [S3 config]} -> {backups, errors}
char *backstack_scan(const char *json);
/// S3 config -> {objects}
char *backstack_list_bucket(const char *json);
/// {source: {path, s3?}, kind} -> Details
char *backstack_inspect(const char *json);
/// {} -> [{name, image, published}]
char *backstack_containers(const char *json);
/// target -> [Database]
char *backstack_databases(const char *json);
/// {target, database} -> {ok}
char *backstack_drop(const char *json);
/// {target, source: {path, s3?}, kind, database, replace} -> {job}
char *backstack_restore(const char *json);
/// {job} -> {phase, bytes, total, finished, error}
char *backstack_job(const char *json);
/// {job} -> {ok}
char *backstack_cancel(const char *json);
void backstack_free(char *s);
