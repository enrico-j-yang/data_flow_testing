class Base:
    def value(self):
        return 1


class Child(Base):
    def value(self):
        return super().value()
