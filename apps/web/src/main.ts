import { mount } from 'svelte';
import App from './App.svelte';
import './app.css';

// Existing browser sessions on the old origin and local development installs.
const previousName = localStorage.getItem('quipcall.name');
if (previousName !== null) {
  if (localStorage.getItem('cubie.name') === null) localStorage.setItem('cubie.name', previousName);
  localStorage.removeItem('quipcall.name');
}

export default mount(App, { target: document.getElementById('app')! });
