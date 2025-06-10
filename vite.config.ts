import { defineConfig } from "vite";
import path from "node:path";

// https://vitejs.dev/config/
export default defineConfig({
	build: {
		lib: {
			entry: ["css/room.css", "css/room_list.css", "js/room_list.js"].map((i) =>
				path.resolve(__dirname, i),
			),
			formats: ["es"],
		},
		cssCodeSplit: true,
		manifest: true,
	},
	resolve: {
		alias: {
			"@": path.resolve(__dirname, "./js"),
			$: path.resolve(__dirname, "./"),
		},
	},
});
