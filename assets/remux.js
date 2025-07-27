
export async function playShaka(sourceUrl, textTracks = []) {
    await initShaka("video-player"); // <- this is now async

    const player = window.shaka_player;
    if (!player) {
        console.error("Shaka player not initialized");
        return;
    }

    const video = player.getMediaElement();
    await player.detach();
    await player.attach(video, true);

    try {
      


      
        await player.load(sourceUrl);
        console.log("Shaka load successful");
//player.addTextTrackAsync(
//  'https://subs5.strem.io/en/download/subencoding-stremio-utf8/src-api/file/54047.srt',
//  'fr', 'subtitles', 'text/srt'
//).then(() => {
//  console.log('Subtitle track added');
//});

//for (const track of textTracks) {
//            player.addTextTrackAsync(
//                track.url,
//                track.lang,
//                'subtitles',
//                track.mime,
//                track.label,
//            ).then(() => {
//});
//        }
        //console.log("Subtitle track added");

        if (textTracks.length > 0) {
           player.setTextTrackVisibility(true);
        }
        
        await video.play().catch(e => console.warn("Autoplay blocked", e));
    } catch (e) {
        console.error("Shaka load failed", e);
    }
}

//window._shaka_player = null;

export async function initShaka(videoId) {
    if (window.shaka_player) {
        console.debug("Shaka already initialized");
        return;
    }

    shaka.polyfill.installAll();

    const video = document.getElementById(videoId);
    if (!video) {
        console.error("Video element not found");
        throw new Error("Video element not found");
    }

    if (!shaka.Player.isBrowserSupported()) {
        console.error("Shaka Player not supported");
        throw new Error("Shaka Player not supported");
    }

    const player = new shaka.Player();
    player.addEventListener('error', e => console.error('Shaka error', e));
    console.debug("Attaching shaka player...");

    await player.attach(video, true);
    console.debug("Shaka player attached");

    window.shaka_player = player;
}

export function getScrollInfo(id) {
    const el = document.getElementById(id);
    if (!el) return null;

    return {
        scrollTop: el.scrollTop,
        scrollLeft: el.scrollLeft,
        scrollWidth: el.scrollWidth,
        scrollHeight: el.scrollHeight,
        clientWidth: el.clientWidth,
        clientHeight: el.clientHeight,
        offsetWidth: el.offsetWidth,
        offsetHeight: el.offsetHeight,
    };
}

export function getWindowSize() {
    return {
        width: window.innerWidth,
        height: window.innerHeight
    };
}

export function findLastPartiallyVisibleIndex(id, direction) {
    const container = document.getElementById(id);
    if (!container) return 0;

    const containerRect = container.getBoundingClientRect();
    const children = container.children;

    let lastVisible = 0;

    for (let i = 0; i < children.length; i++) {
        const child = children[i];
        const childRect = child.getBoundingClientRect();

        let overlap = 0;

        if (direction === "horizontal") {
            const visibleLeft = Math.max(childRect.left, containerRect.left);
            const visibleRight = Math.min(childRect.right, containerRect.right);
            overlap = visibleRight - visibleLeft;
        } else {
            const visibleTop = Math.max(childRect.top, containerRect.top);
            const visibleBottom = Math.min(childRect.bottom, containerRect.bottom);
            overlap = visibleBottom - visibleTop;
        }

        const isVisible = overlap > 5;

        if (isVisible) {
            lastVisible = i;
        }
    }

    return lastVisible;
}