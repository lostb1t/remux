// @generated automatically by Diesel CLI.

diesel::table! {
    auth_devices (user_id, id) {
        user_id -> Text,
        id -> Text,
        access_token -> Text,
        name -> Text,
        app_name -> Text,
        app_version -> Text,
    }
}

diesel::table! {
    auth_users (id) {
        id -> Text,
        username -> Text,
        password_hash -> Text,
        aio_url -> Nullable<Text>,
    }
}

diesel::table! {
    media (id) {
        id -> Text,
        title -> Text,
        kind -> Text,
        parent_id -> Nullable<Text>,
        idx -> Nullable<Integer>,
        released_at -> Nullable<Timestamp>,
        runtime -> Nullable<Integer>,
        rating_critic -> Nullable<Integer>,
        rating_audience -> Nullable<Integer>,
        poster -> Nullable<Text>,
        url -> Nullable<Text>,
        probe_data -> Nullable<Text>,
        remote_data -> Nullable<Text>,
        created_at -> Timestamp,
        updated_at -> Timestamp,
    }
}

diesel::table! {
    provider_ids (media_id, kind) {
        media_id -> Text,
        kind -> Text,
        id -> Text,
    }
}

diesel::joinable!(provider_ids -> media (media_id));

diesel::allow_tables_to_appear_in_same_query!(
    auth_devices,
    auth_users,
    media,
    provider_ids,
);
