const fileInput = document.getElementById('fileInput');
const dropZone = document.getElementById('dropZone');
const uploadSection = document.getElementById('uploadSection');
const statusSection = document.getElementById('statusSection');
const playerSection = document.getElementById('playerSection');
const uploadProgressContainer = document.getElementById('uploadProgressContainer');
const uploadProgressBar = document.getElementById('uploadProgressBar');
const uploadStatusText = document.getElementById('uploadStatusText');
const jobStatusText = document.getElementById('jobStatusText');

const R2_PUBLIC_BASE = 'http://localhost:4566/video-uploads';

// Read query params for direct sharing link
document.addEventListener('DOMContentLoaded', () => {
    const params = new URLSearchParams(window.location.search);
    const sharedVideoId = params.get('v');
    if (sharedVideoId) {
        pollStatus(sharedVideoId);
    }
});

// Drag & Drop Styling
dropZone.addEventListener('dragover', (e) => {
    e.preventDefault();
    dropZone.classList.add('dragover');
});
dropZone.addEventListener('dragleave', () => dropZone.classList.remove('dragover'));
dropZone.addEventListener('drop', (e) => {
    e.preventDefault();
    dropZone.classList.remove('dragover');
    if (e.dataTransfer.files.length > 0) {
        handleUpload(e.dataTransfer.files[0]);
    }
});
fileInput.addEventListener('change', (e) => {
    if (e.target.files.length > 0) {
        handleUpload(e.target.files[0]);
    }
});

async function handleUpload(file) {
    if (!file.type.startsWith('video/')) {
        alert('Please select a video file.');
        return;
    }

    try {
        //  Get Presigned URL
        uploadStatusText.innerText = "Requesting upload URL...";
        const res = await fetch('/api/upload/url');
        const data = await res.json();

        if (!data.upload_url || !data.video_id) throw new Error("Invalid URL response");

        // UI Updates
        dropZone.classList.add('hidden');
        uploadProgressContainer.classList.remove('hidden');
        uploadStatusText.innerText = "Uploading directly to storage...";

        //  Perform PUT Request tracking progress via XMLHttpRequest
        await new Promise((resolve, reject) => {
            const xhr = new XMLHttpRequest();
            xhr.open('PUT', data.upload_url, true);

            xhr.upload.onprogress = (e) => {
                if (e.lengthComputable) {
                    const percentComplete = (e.loaded / e.total) * 100;
                    uploadProgressBar.style.width = percentComplete + '%';
                }
            };

            xhr.onload = () => {
                if (xhr.status >= 200 && xhr.status < 300) {
                    resolve();
                } else {
                    reject(new Error("Upload failed"));
                }
            };
            xhr.onerror = () => reject(new Error("Network error"));

            xhr.send(file);
        });

        uploadStatusText.innerText = "Upload complete! Triggers processing...";

        // Emulate R2 Webhook trigger manually for localstack because localstack S3 event bridge 
        // to our API requires explicit wiring which might not be fully configured in init script
        try {
            uploadStatusText.innerText = "Simulating Cloudflare Webhook...";
            await fetch('/api/webhook/r2', {
                method: "POST",
                headers: { "Content-Type": "application/json" },
                body: JSON.stringify({
                    file_name: `${data.video_id}.mp4`,
                    size_bytes: file.size
                })
            });
        } catch (e) {
            console.error("Webhook trigger failed, assuming automatic hook active", e);
        }

        // Move to polling
        pollStatus(data.video_id);

    } catch (error) {
        console.error(error);
        uploadStatusText.innerText = "Error: " + error.message;
        dropZone.classList.remove('hidden');
        uploadProgressContainer.classList.add('hidden');
    }
}

async function pollStatus(videoId) {
    uploadSection.classList.add('hidden');
    statusSection.classList.remove('hidden');

    const interval = setInterval(async () => {
        try {
            const res = await fetch('/api/status/' + videoId);
            if (!res.ok) return;

            const data = await res.json();

            if (data.status === 'processing') {
                jobStatusText.innerText = "Segmenting & Parallel Transcoding in progress...";
            } else if (data.status === 'ready') {
                clearInterval(interval);
                showPlayer(videoId, data.url || `${R2_PUBLIC_BASE}/transcoded/${videoId}/master.m3u8`);
            }
        } catch (e) {
            console.error('Polling error', e);
        }
    }, 2000);
}

function showPlayer(videoId, videoUrl) {
    statusSection.classList.add('hidden');
    playerSection.classList.remove('hidden');

    const videoElement = document.getElementById('videoPlayer');
    const shareUrlInput = document.getElementById('shareUrlInput');
    const copyUrlBtn = document.getElementById('copyUrlBtn');
    const resolutionSelect = document.getElementById('resolutionSelect');

    // Create the fully qualified app URL
    const appShareUrl = `${window.location.origin}/?v=${videoId}`;

    // Populate the Shareable URL Box
    shareUrlInput.value = appShareUrl;
    
    // Clear old listeners by cloning the node (prevents duplicating copy events)
    const newCopyBtn = copyUrlBtn.cloneNode(true);
    copyUrlBtn.parentNode.replaceChild(newCopyBtn, copyUrlBtn);

    newCopyBtn.addEventListener('click', () => {
        navigator.clipboard.writeText(appShareUrl).then(() => {
            newCopyBtn.innerText = "Copied!";
            setTimeout(() => newCopyBtn.innerText = "Copy Link", 2000);
        });
    });

    if (Hls.isSupported()) {
        const hls = new Hls({
            lowLatencyMode: true,
        });
        let isRefreshing = false;

        hls.on(Hls.Events.MANIFEST_PARSED, function () {
            console.log("Manifest parsed! Quality levels:", hls.levels.map(l => l.height + "p"));
            
            // Clear prior options
            resolutionSelect.innerHTML = '';
            const autoOpt = document.createElement('option');
            autoOpt.value = -1;
            autoOpt.text = 'Auto';
            resolutionSelect.appendChild(autoOpt);

            // Populate our new Resolution Dropdown
            hls.levels.forEach((level, index) => {
                const opt = document.createElement('option');
                opt.value = index;
                opt.text = level.height + 'p';
                resolutionSelect.appendChild(opt);
            });

            // Listen for manual selection
            resolutionSelect.addEventListener('change', (e) => {
                hls.currentLevel = parseInt(e.target.value);
            });
            
            isRefreshing = false;
        });

        hls.loadSource(videoUrl);
        hls.attachMedia(videoElement);
        videoElement.play().catch(() => { });

        // Background poller to magically upgrade stream quality when HD workers finish
        window.hdCheckInterval = setInterval(async () => {
            if (isRefreshing) return;
            try {
                const res = await fetch(videoUrl + '?t=' + Date.now());
                const text = await res.text();
                const matches = text.match(/RESOLUTION=/g);
                const availableLevelsCount = matches ? matches.length : 0;
                
                // If the master playlist has MORE resolutions than hls.js currently knows about
                if (availableLevelsCount > hls.levels.length) {
                    console.log("New HD resolutions detected! Upgrading stream in place...");
                    isRefreshing = true;
                    const currentTime = videoElement.currentTime;
                    const isPlaying = !videoElement.paused;
                    
                    // Tell HLS to swap the underlying pipeline
                    hls.loadSource(videoUrl);
                    
                    hls.once(Hls.Events.MANIFEST_PARSED, () => {
                        videoElement.currentTime = currentTime;
                        if (isPlaying) videoElement.play().catch(()=>{});
                    });

                    // Stop polling once we safely hit 3 variants (1080p, 720p, 480p)
                    if (availableLevelsCount >= 3) {
                        clearInterval(window.hdCheckInterval);
                    }
                }
            } catch(e) {}
        }, 5000);
    } else if (videoElement.canPlayType('application/vnd.apple.mpegurl')) {
        // Native Apple Safari fallback
        videoElement.src = videoUrl;
        videoElement.play();
    } else {
        alert("Your browser does not support HLS.");
    }
}
