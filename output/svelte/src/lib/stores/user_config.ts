import { writable } from 'svelte/store';
import { browser } from '$app/environment';


export type UserConfig = {

    // Used to recover lines up to a certain point
    line_checkpoint_index: number

    // Keep track of the lines this user has written
    // necessary because user written lines will always be fetched
    lines: string[] // list of date as ISO string
}

const DEFAULT_VALUE: UserConfig = {
    line_checkpoint_index: 0,
    lines: []
}

function load_config() {
    let config = DEFAULT_VALUE;
    if (browser && localStorage.getItem('user_config')) {
        config = JSON.parse(localStorage.getItem('user_config') || '') as UserConfig;
    }

    // Check for missing keys
    for (const key in DEFAULT_VALUE) {
        if (!(key in config)) {
            // @ts-ignore
            config[key] = DEFAULT_VALUE[key];
        }
    }

    return config;

}


const initial_value: UserConfig = load_config();
export const user_config = writable(initial_value);

user_config.subscribe((value) => {
    if (browser) {
        localStorage.setItem('user_config', JSON.stringify(value));
    }
});

export default user_config;