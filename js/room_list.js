const directRoomsBtn = document.getElementById("direct-rooms");
const encryptedRoomsBtn = document.getElementById("encrypted-rooms");
const unreadRoomsBtn = document.getElementById("unread-rooms");
const roomCards = document.querySelectorAll(".room-card");

// Filter states: null (disabled), true (enabled), false (inverted)
const filters = {
	direct: null,
	encrypted: null,
	unread: null,
};

// Add CSS classes to buttons based on filter state
function updateButtonStyles(btn, filter) {
	btn.classList.remove("active", "inverted");
	if (filter === true) btn.classList.add("active");
	if (filter === false) btn.classList.add("inverted");
}

function checkFilter(filter, value) {
	if (filter === null) return true;
	return filter === value;
}

function applyFilters() {
	for (const card of roomCards) {
		const showCard =
			checkFilter(filters.direct, card.dataset.isDirect === "true") &&
			checkFilter(filters.encrypted, card.dataset.isEncrypted === "true") &&
			checkFilter(filters.unread, card.dataset.hasUnread === "true");
		// Show or hide the card
		card.style.display = showCard ? "flex" : "none";
	}
}

// Toggle filter state: null -> true -> false -> null
function toggleFilter(filterName, btn) {
	if (filters[filterName] === null) {
		filters[filterName] = true;
	} else if (filters[filterName] === true) {
		filters[filterName] = false;
	} else {
		filters[filterName] = null;
	}

	updateButtonStyles(btn, filters[filterName]);
	applyFilters();
}

// Set up event listeners for filter buttons
directRoomsBtn.addEventListener("click", () => {
	toggleFilter("direct", directRoomsBtn);
});

encryptedRoomsBtn.addEventListener("click", () => {
	toggleFilter("encrypted", encryptedRoomsBtn);
});

unreadRoomsBtn.addEventListener("click", () => {
	toggleFilter("unread", unreadRoomsBtn);
});
