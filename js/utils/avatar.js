function handleAvatarImageLoad(imageSelector = '.avatar-image', containerSelector = '.avatar-container') {
    const avatarImages = document.querySelectorAll(imageSelector);

    avatarImages.forEach(img => {
        const avatarContainer = img.closest(containerSelector);

        if (!avatarContainer) {
            return;
        }

        if (img.complete && img.naturalWidth > 0) {
            // Image is already loaded
            avatarContainer.classList.add('avatar-loaded');
        } else {
            img.addEventListener('load', function() {
                avatarContainer.classList.add('avatar-loaded');
            });

            img.addEventListener('error', function() {
                avatarContainer.classList.remove('avatar-loaded');
            });
        }
    });
}

function initAvatars() {
    handleAvatarImageLoad('.avatar-image', '.avatar-container');
}

// Export functions for use in other modules
export {
    initAvatars
};
