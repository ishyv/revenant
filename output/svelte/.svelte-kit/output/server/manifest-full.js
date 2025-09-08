export const manifest = (() => {
function __memo(fn) {
	let value;
	return () => value ??= (value = fn());
}

return {
	appDir: "_app",
	appPath: "_app",
	assets: new Set(["et-book/et-book-bold-line-figures/et-book-bold-line-figures.eot","et-book/et-book-bold-line-figures/et-book-bold-line-figures.svg","et-book/et-book-bold-line-figures/et-book-bold-line-figures.ttf","et-book/et-book-bold-line-figures/et-book-bold-line-figures.woff","et-book/et-book-display-italic-old-style-figures/et-book-display-italic-old-style-figures.eot","et-book/et-book-display-italic-old-style-figures/et-book-display-italic-old-style-figures.svg","et-book/et-book-display-italic-old-style-figures/et-book-display-italic-old-style-figures.ttf","et-book/et-book-display-italic-old-style-figures/et-book-display-italic-old-style-figures.woff","et-book/et-book-roman-line-figures/et-book-roman-line-figures.eot","et-book/et-book-roman-line-figures/et-book-roman-line-figures.svg","et-book/et-book-roman-line-figures/et-book-roman-line-figures.ttf","et-book/et-book-roman-line-figures/et-book-roman-line-figures.woff","et-book/et-book-roman-old-style-figures/et-book-roman-old-style-figures.eot","et-book/et-book-roman-old-style-figures/et-book-roman-old-style-figures.svg","et-book/et-book-roman-old-style-figures/et-book-roman-old-style-figures.ttf","et-book/et-book-roman-old-style-figures/et-book-roman-old-style-figures.woff","et-book/et-book-semi-bold-old-style-figures/et-book-semi-bold-old-style-figures.eot","et-book/et-book-semi-bold-old-style-figures/et-book-semi-bold-old-style-figures.svg","et-book/et-book-semi-bold-old-style-figures/et-book-semi-bold-old-style-figures.ttf","et-book/et-book-semi-bold-old-style-figures/et-book-semi-bold-old-style-figures.woff","robots.txt"]),
	mimeTypes: {".svg":"image/svg+xml",".ttf":"font/ttf",".woff":"font/woff",".txt":"text/plain"},
	_: {
		client: {start:"_app/immutable/entry/start.C9sCpalO.js",app:"_app/immutable/entry/app.D2B7IloV.js",imports:["_app/immutable/entry/start.C9sCpalO.js","_app/immutable/chunks/CVmrHqaL.js","_app/immutable/chunks/24IvBYb4.js","_app/immutable/chunks/CD26wNPw.js","_app/immutable/chunks/CLJoI33R.js","_app/immutable/entry/app.D2B7IloV.js","_app/immutable/chunks/CD26wNPw.js","_app/immutable/chunks/24IvBYb4.js","_app/immutable/chunks/CLJoI33R.js","_app/immutable/chunks/DsnmJJEf.js"],stylesheets:[],fonts:[],uses_env_dynamic_public:false},
		nodes: [
			__memo(() => import('./nodes/0.js')),
			__memo(() => import('./nodes/1.js')),
			__memo(() => import('./nodes/2.js'))
		],
		remotes: {
			
		},
		routes: [
			{
				id: "/",
				pattern: /^\/$/,
				params: [],
				page: { layouts: [0,], errors: [1,], leaf: 2 },
				endpoint: null
			}
		],
		prerendered_routes: new Set([]),
		matchers: async () => {
			
			return {  };
		},
		server_assets: {}
	}
}
})();
