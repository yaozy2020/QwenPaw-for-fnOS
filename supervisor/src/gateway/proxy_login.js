const toggle = document.querySelector('#reveal');
const field = document.querySelector('#password');
if (toggle && field) {
  toggle.hidden = false;
  toggle.onclick = () => {
    const show = field.type === 'password';
    field.type = show ? 'text' : 'password';
    toggle.textContent = show ? '隐藏' : '显示';
    toggle.setAttribute('aria-pressed', String(show));
  };
}
