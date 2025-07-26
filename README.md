
<div align="center">
   <img width="200" height="200" src="logo.png" alt="Logo">
</div>
   
<div align="center">
  <h1><b>Remux</b></h1>
  <p><i>A *VERY experimental* web/desktop client for Jellyfin written in Rust </i></p>
</div>

<details>
<summary> Mobile Layout </summary>
  


<p align="center">
  <img src="mobile.png" style="max-width:500px; width:45%" alt="Image 1">
  <img src="mobile.png" style="max-width:500px; width:45%" alt="Image 2">
</p>

</details>

<details>
<summary> Desktop Layout </summary>
  
![Desktop](desktop.png)

</details>

Hosted version at: https://app.remux.media

This only works for Jellyfin servers that are behind a reverse proxy and have HTTPS set up correctly. If your server runs over HTTP, you must host it yourself.

### Catalogs

The home screen is heavily reliant on collections. 
So i suggest using some jellyfin plugins to create some cool ones.

You can manage the home screen from the settings.

### Why another client?

For fun and learning Rust. And ofcourse the usual delusions of "i can do it better".

What makes this different.

- A more family orientated client. Most users just want that netflix experience.
- Multiple providers support. It has a pluggable source system. So while only jellyfin is currently supported im also planning support for stremio
- central management of settings for all users
- tons of options to customize the home screen

### Selfhosting
 
Theres a docker image avaiable at: ghcr.io/lostb1t/remux:latest

Experimental native builds can be found here: https://github.com/lostb1t/remux/releases/tag/nightly

### Development

Make sure you have installed [Rust](https://www.rust-lang.org/tools/install)

1. Install the Tailwind CSS CLI: https://tailwindcss.com/docs/installation/tailwind-cli
2. Install rust dependencies: `cargo install cargo-make dioxus-cli`
3. run dev: `cargo make dev`
