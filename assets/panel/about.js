  // Seção Sobre do NeuralIA: identificação, dados institucionais e checagem de atualizações.
  const about = (() => {
    const updateBtn = byId('about-update-btn');
    if (updateBtn) {
      updateBtn.addEventListener('click', () => {
        post('check-update');
      });
    }

    const linkIds = ['about-link-linkedin', 'about-link-github', 'about-link-huggingface'];
    for (let i = 0; i < linkIds.length; i++) {
      const btn = byId(linkIds[i]);
      if (btn) {
        btn.addEventListener('click', () => {
          const url = btn.getAttribute('data-url');
          if (url) post('open', { input: url });
        });
      }
    }

    return {};
  })();
